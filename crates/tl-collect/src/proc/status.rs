//! Parser for /proc/<pid>/status (PROC-002, PROC-028, PROC-030, PROC-078, PROC-082, D-004,
//! R-005, ADR-015).
//!
//! The file is world-readable. Each line is `Key:<tabs or spaces>value`. Unknown keys are
//! ignored. Memory lines are in kB and are stored in bytes.

use memchr::memchr;
use smallvec::SmallVec;

use crate::CollectError;

const FILE: &str = "status";

/// The caller reads at most this many bytes (ADR-015, same as `sanitize::STATUS_MAX`). A longer
/// file is parsed up to the cut.
pub const STATUS_READ_MAX: usize = 8 * 1024;

/// The fields of one status file that tasklens uses. Sizes are in bytes. A memory field is
/// `None` when its line is absent (kernel threads have no `Vm*` or `Rss*` lines).
/// Reuse one value across processes so the small vectors keep their capacity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PidStatus {
    /// `Uid:` real, effective, saved, fs.
    pub uid: [u32; 4],
    /// `Gid:` real, effective, saved, fs.
    pub gid: [u32; 4],
    /// `Groups:` supplementary groups.
    pub groups: SmallVec<[u32; 16]>,
    /// `Threads:`
    pub threads: u32,
    /// `RssAnon:` the default Memory column (D-004).
    pub rss_anon: Option<u64>,
    /// `RssFile:`
    pub rss_file: Option<u64>,
    /// `RssShmem:`
    pub rss_shmem: Option<u64>,
    /// `VmRSS:`
    pub vm_rss: Option<u64>,
    /// `VmSwap:`
    pub vm_swap: Option<u64>,
    /// `VmHWM:` peak resident set.
    pub vm_hwm: Option<u64>,
    /// `VmPeak:` peak virtual size.
    pub vm_peak: Option<u64>,
    /// `VmSize:`
    pub vm_size: Option<u64>,
    /// `VmData:`
    pub vm_data: Option<u64>,
    /// `VmStk:`
    pub vm_stk: Option<u64>,
    /// `VmPTE:`
    pub vm_pte: Option<u64>,
    /// `Cpus_allowed_list:` raw text, for example `0-7,16-23`.
    pub cpus_allowed_list: SmallVec<[u8; 32]>,
    /// `CapInh:`
    pub cap_inh: u64,
    /// `CapPrm:`
    pub cap_prm: u64,
    /// `CapEff:`
    pub cap_eff: u64,
    /// `CapBnd:`
    pub cap_bnd: u64,
    /// `CapAmb:`
    pub cap_amb: u64,
    /// `NoNewPrivs:`
    pub no_new_privs: Option<bool>,
    /// `Seccomp:` 0 disabled, 1 strict, 2 filter.
    pub seccomp: Option<u8>,
    /// `Seccomp_filters:`
    pub seccomp_filters: Option<u32>,
    /// `Speculation_Store_Bypass:` raw text.
    pub speculation_store_bypass: SmallVec<[u8; 32]>,
    /// `SpeculationIndirectBranch:` raw text.
    pub speculation_indirect_branch: SmallVec<[u8; 32]>,
    /// `x86_Thread_features:` raw text, `None` when the line is absent.
    pub x86_thread_features: Option<SmallVec<[u8; 32]>>,
    /// `voluntary_ctxt_switches:`
    pub voluntary_ctxt_switches: Option<u64>,
    /// `nonvoluntary_ctxt_switches:`
    pub nonvoluntary_ctxt_switches: Option<u64>,
}

impl PidStatus {
    /// Sets every field to its default and keeps the capacity of the small vectors.
    fn reset(&mut self) {
        let mut groups = std::mem::take(&mut self.groups);
        let mut cpus = std::mem::take(&mut self.cpus_allowed_list);
        let mut ssb = std::mem::take(&mut self.speculation_store_bypass);
        let mut ib = std::mem::take(&mut self.speculation_indirect_branch);
        groups.clear();
        cpus.clear();
        ssb.clear();
        ib.clear();
        *self = Self {
            groups,
            cpus_allowed_list: cpus,
            speculation_store_bypass: ssb,
            speculation_indirect_branch: ib,
            ..Self::default()
        };
    }
}

/// Parses one status file into `out`. `out` is reset first. A missing or malformed `Uid:` line
/// is `ProcParse { pid: 0, file: "status" }`; the caller fills in the pid. Other malformed
/// values are treated as absent.
///
/// When `buf` is `STATUS_READ_MAX` bytes or longer, the read was cut, so a last line without a
/// newline is incomplete and is skipped.
pub fn parse_status(buf: &[u8], out: &mut PidStatus) -> Result<(), CollectError> {
    out.reset();
    let mut have_uid = false;
    let mut rest = buf;
    while !rest.is_empty() {
        let line = match memchr(b'\n', rest) {
            Some(i) => {
                let line = &rest[..i];
                rest = &rest[i + 1..];
                line
            }
            None if buf.len() >= STATUS_READ_MAX => break,
            None => std::mem::take(&mut rest),
        };
        let Some(colon) = memchr(b':', line) else {
            continue;
        };
        let value = trim(&line[colon + 1..]);
        match &line[..colon] {
            b"Uid" => have_uid = parse_ids(value, &mut out.uid),
            b"Gid" => {
                parse_ids(value, &mut out.gid);
            }
            b"Groups" => {
                for token in tokens(value) {
                    if let Some(g) = unsigned(token) {
                        out.groups.push(g);
                    }
                }
            }
            b"Threads" => out.threads = unsigned(value).unwrap_or(0),
            b"RssAnon" => out.rss_anon = kb(value),
            b"RssFile" => out.rss_file = kb(value),
            b"RssShmem" => out.rss_shmem = kb(value),
            b"VmRSS" => out.vm_rss = kb(value),
            b"VmSwap" => out.vm_swap = kb(value),
            b"VmHWM" => out.vm_hwm = kb(value),
            b"VmPeak" => out.vm_peak = kb(value),
            b"VmSize" => out.vm_size = kb(value),
            b"VmData" => out.vm_data = kb(value),
            b"VmStk" => out.vm_stk = kb(value),
            b"VmPTE" => out.vm_pte = kb(value),
            b"Cpus_allowed_list" => out.cpus_allowed_list.extend_from_slice(value),
            b"CapInh" => out.cap_inh = hex(value),
            b"CapPrm" => out.cap_prm = hex(value),
            b"CapEff" => out.cap_eff = hex(value),
            b"CapBnd" => out.cap_bnd = hex(value),
            b"CapAmb" => out.cap_amb = hex(value),
            b"NoNewPrivs" => {
                out.no_new_privs = match value {
                    b"0" => Some(false),
                    b"1" => Some(true),
                    _ => None,
                };
            }
            b"Seccomp" => out.seccomp = unsigned(value),
            b"Seccomp_filters" => out.seccomp_filters = unsigned(value),
            b"Speculation_Store_Bypass" => out.speculation_store_bypass.extend_from_slice(value),
            b"SpeculationIndirectBranch" => {
                out.speculation_indirect_branch.extend_from_slice(value);
            }
            b"x86_Thread_features" => {
                out.x86_thread_features = Some(SmallVec::from_slice(value));
            }
            b"voluntary_ctxt_switches" => out.voluntary_ctxt_switches = unsigned(value),
            b"nonvoluntary_ctxt_switches" => out.nonvoluntary_ctxt_switches = unsigned(value),
            _ => {}
        }
    }
    if have_uid {
        Ok(())
    } else {
        Err(CollectError::ProcParse { pid: 0, file: FILE })
    }
}

/// PROC-030 Commit size: `VmData + VmStk`, `None` if either is absent.
pub fn commit_bytes(s: &PidStatus) -> Option<u64> {
    s.vm_data?.checked_add(s.vm_stk?)
}

/// Working set shared: `RssFile + RssShmem`, `None` if either is absent.
pub fn ws_shared_bytes(s: &PidStatus) -> Option<u64> {
    s.rss_file?.checked_add(s.rss_shmem?)
}

fn trim(value: &[u8]) -> &[u8] {
    let is_blank = |b: &u8| *b == b' ' || *b == b'\t';
    let start = value.iter().position(|b| !is_blank(b)).unwrap_or(value.len());
    let end = value.iter().rposition(|b| !is_blank(b)).map_or(start, |i| i + 1);
    &value[start..end]
}

fn tokens(value: &[u8]) -> impl Iterator<Item = &[u8]> {
    value
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|t| !t.is_empty())
}

/// Four ids separated by tabs. Returns false and leaves `out` alone unless all four parse.
fn parse_ids(value: &[u8], out: &mut [u32; 4]) -> bool {
    let mut ids = [0u32; 4];
    let mut it = tokens(value);
    for id in &mut ids {
        match it.next().and_then(unsigned) {
            Some(n) => *id = n,
            None => return false,
        }
    }
    *out = ids;
    true
}

/// `1234 kB` as bytes.
fn kb(value: &[u8]) -> Option<u64> {
    let digits = value.strip_suffix(b"kB").map_or(value, trim);
    parse_u64(digits)?.checked_mul(1024)
}

/// A hex mask. Malformed text is 0.
fn hex(value: &[u8]) -> u64 {
    std::str::from_utf8(value)
        .ok()
        .and_then(|s| u64::from_str_radix(s, 16).ok())
        .unwrap_or(0)
}

fn unsigned<T: TryFrom<u64>>(token: &[u8]) -> Option<T> {
    T::try_from(parse_u64(token)?).ok()
}

/// Decimal digits only, at least one, no overflow.
fn parse_u64(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u64, |n, &d| {
        if d.is_ascii_digit() {
            n.checked_mul(10)?.checked_add(u64::from(d - b'0'))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_helpers() {
        assert_eq!(trim(b" \t 12 kB\t "), b"12 kB");
        assert_eq!(trim(b" \t"), b"");
        assert_eq!(kb(b"12 kB"), Some(12 * 1024));
        assert_eq!(kb(b"0 kB"), Some(0));
        assert_eq!(kb(b"kB"), None);
        assert_eq!(kb(b"18014398509481984 kB"), None);
        assert_eq!(hex(b"000001ffffffffff"), 0x1ff_ffff_ffff);
        assert_eq!(hex(b"zz"), 0);
    }

    #[test]
    fn reuse_clears_old_values() {
        let mut out = PidStatus::default();
        parse_status(
            b"Uid:\t1\t2\t3\t4\nGroups:\t5 6\nRssAnon:\t8 kB\nCpus_allowed_list:\t0-3\nx86_Thread_features:\tshstk\n",
            &mut out,
        )
        .unwrap();
        assert_eq!(out.groups.as_slice(), [5, 6]);
        assert_eq!(out.x86_thread_features.as_deref(), Some(&b"shstk"[..]));
        parse_status(b"Uid:\t9\t9\t9\t9\n", &mut out).unwrap();
        assert_eq!(out.uid, [9; 4]);
        assert!(out.groups.is_empty());
        assert!(out.cpus_allowed_list.is_empty());
        assert_eq!(out.rss_anon, None);
        assert_eq!(out.x86_thread_features, None);
    }

    #[test]
    fn short_buffer_parses_last_line_without_newline() {
        let mut out = PidStatus::default();
        parse_status(b"Uid:\t1\t2\t3\t4\nThreads:\t7", &mut out).unwrap();
        assert_eq!(out.threads, 7);
    }
}
