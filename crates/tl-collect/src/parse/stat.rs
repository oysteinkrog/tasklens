//! Parser for /proc/stat: the `cpu` and `cpuN` lines and the scalar lines (ADR-008, ADR-016).

use memchr::memchr;

use crate::CollectError;

const FILE: &str = "/proc/stat";

const CPU_LINE: CollectError = CollectError::Parse {
    file: FILE,
    what: "cpu line",
};
const CPU_INDEX: CollectError = CollectError::Parse {
    file: FILE,
    what: "cpu index",
};
const NO_CPU_LINE: CollectError = CollectError::Parse {
    file: FILE,
    what: "no cpu line",
};
const NUMBER: CollectError = CollectError::Parse {
    file: FILE,
    what: "number",
};

/// Clock ticks (USER_HZ) from one cpu line. Field order as in /proc/stat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
    pub guest: u64,
    pub guest_nice: u64,
}

impl CpuTimes {
    /// user + nice + system + irq + softirq + steal.
    pub fn busy(&self) -> u64 {
        self.user
            .wrapping_add(self.nice)
            .wrapping_add(self.system)
            .wrapping_add(self.irq)
            .wrapping_add(self.softirq)
            .wrapping_add(self.steal)
    }

    /// busy + idle + iowait. guest and guest_nice are left out: the kernel already counts them in
    /// user and nice.
    pub fn total(&self) -> u64 {
        self.busy()
            .wrapping_add(self.idle)
            .wrapping_add(self.iowait)
    }

    /// system + irq + softirq.
    pub fn kernel(&self) -> u64 {
        self.system
            .wrapping_add(self.irq)
            .wrapping_add(self.softirq)
    }

    /// irq + softirq.
    pub fn interrupts(&self) -> u64 {
        self.irq.wrapping_add(self.softirq)
    }

    fn from_fields(f: &[u64; 10]) -> Self {
        Self {
            user: f[0],
            nice: f[1],
            system: f[2],
            idle: f[3],
            iowait: f[4],
            irq: f[5],
            softirq: f[6],
            steal: f[7],
            guest: f[8],
            guest_nice: f[9],
        }
    }
}

/// One parse of /proc/stat. Reuse it across samples so `per_cpu` keeps its capacity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatData {
    pub total: CpuTimes,
    /// (N from "cpuN", times), in file order.
    pub per_cpu: Vec<(u16, CpuTimes)>,
    pub ctxt: u64,
    pub btime: u64,
    pub processes: u64,
    pub procs_running: u32,
    pub procs_blocked: u32,
}

/// Parses `buf` into `out` in one pass. `out.per_cpu` is cleared and refilled, so a second parse
/// of a same-size file allocates nothing.
pub fn parse_stat(buf: &[u8], out: &mut StatData) -> Result<(), CollectError> {
    out.total = CpuTimes::default();
    out.per_cpu.clear();
    out.ctxt = 0;
    out.btime = 0;
    out.processes = 0;
    out.procs_running = 0;
    out.procs_blocked = 0;

    let mut seen_total = false;
    let mut pos = 0;
    while pos < buf.len() {
        let end = memchr(b'\n', &buf[pos..]).map_or(buf.len(), |i| pos + i);
        let line = &buf[pos..end];
        pos = end + 1;

        let (key, rest) = split_key(line);
        match key {
            // `intr` has hundreds of fields and `softirq` is not needed: skip both unparsed.
            b"intr" | b"softirq" => {}
            b"cpu" => {
                out.total = parse_cpu_fields(rest)?;
                seen_total = true;
            }
            b"ctxt" => out.ctxt = parse_scalar(rest)?,
            b"btime" => out.btime = parse_scalar(rest)?,
            b"processes" => out.processes = parse_scalar(rest)?,
            b"procs_running" => out.procs_running = parse_scalar_u32(rest)?,
            b"procs_blocked" => out.procs_blocked = parse_scalar_u32(rest)?,
            _ => {
                if let Some(digits) = key.strip_prefix(b"cpu")
                    && !digits.is_empty()
                    && digits.iter().all(u8::is_ascii_digit)
                {
                    let index = parse_cpu_index(digits)?;
                    out.per_cpu.push((index, parse_cpu_fields(rest)?));
                }
            }
        }
    }

    if seen_total { Ok(()) } else { Err(NO_CPU_LINE) }
}

/// Splits a line at its first space or tab: (key, the rest).
fn split_key(line: &[u8]) -> (&[u8], &[u8]) {
    match line.iter().position(|&b| b == b' ' || b == b'\t') {
        Some(i) => (&line[..i], &line[i..]),
        None => (line, &[]),
    }
}

fn parse_cpu_index(digits: &[u8]) -> Result<u16, CollectError> {
    let mut n: u32 = 0;
    for &d in digits {
        n = n * 10 + u32::from(d - b'0');
        if n > u32::from(u16::MAX) {
            return Err(CPU_INDEX);
        }
    }
    // n <= u16::MAX here.
    u16::try_from(n).map_err(|_| CPU_INDEX)
}

/// Reads up to 10 numbers. Fewer than 4 is an error, missing ones are 0, extra ones are ignored.
fn parse_cpu_fields(rest: &[u8]) -> Result<CpuTimes, CollectError> {
    let mut fields = [0u64; 10];
    let mut count = 0;
    for token in tokens(rest) {
        if count == fields.len() {
            break;
        }
        fields[count] = parse_u64(token)?;
        count += 1;
    }
    if count < 4 {
        return Err(CPU_LINE);
    }
    Ok(CpuTimes::from_fields(&fields))
}

fn parse_scalar(rest: &[u8]) -> Result<u64, CollectError> {
    match tokens(rest).next() {
        Some(token) => parse_u64(token),
        None => Err(NUMBER),
    }
}

fn parse_scalar_u32(rest: &[u8]) -> Result<u32, CollectError> {
    u32::try_from(parse_scalar(rest)?).map_err(|_| NUMBER)
}

/// The space- or tab-separated tokens of `bytes`.
fn tokens(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split(|&b| b == b' ' || b == b'\t')
        .filter(|t| !t.is_empty())
}

/// A decimal u64. Empty, non-digit or overflowing input is `Parse { what: "number" }`.
fn parse_u64(token: &[u8]) -> Result<u64, CollectError> {
    if token.is_empty() {
        return Err(NUMBER);
    }
    let mut n: u64 = 0;
    for &b in token {
        if !b.is_ascii_digit() {
            return Err(NUMBER);
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or(NUMBER)?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::sample::Index;

    const OWNER_DESKTOP: &[u8] = include_bytes!("../../tests/fixtures/owner-desktop/proc/stat");

    /// The owner-desktop file, cut at a random point, with up to 16 bytes flipped and up to 8
    /// separator or digit bytes inserted.
    fn mutated_owner_desktop() -> impl Strategy<Value = Vec<u8>> {
        (
            any::<Index>(),
            prop::collection::vec((any::<Index>(), any::<u8>()), 0..=16),
            prop::collection::vec(
                (
                    any::<Index>(),
                    prop::sample::select(b"\n \tcpu0123456789".as_slice()),
                ),
                0..=8,
            ),
        )
            .prop_map(|(cut, flips, inserts)| {
                let mut bytes = OWNER_DESKTOP.to_vec();
                bytes.truncate(cut.index(bytes.len() + 1));
                if !bytes.is_empty() {
                    for (at, mask) in flips {
                        let i = at.index(bytes.len());
                        bytes[i] ^= mask;
                    }
                }
                for (at, byte) in inserts {
                    let i = at.index(bytes.len() + 1);
                    bytes.insert(i, byte);
                }
                bytes
            })
    }

    proptest! {
        #[test]
        fn prop_parse_stat_never_panics(
            arbitrary in prop::collection::vec(any::<u8>(), 0..=8192),
            mutated in mutated_owner_desktop(),
        ) {
            let mut out = StatData::default();
            let _ = parse_stat(&arbitrary, &mut out);
            let _ = parse_stat(&mutated, &mut out);
        }
    }

    #[test]
    fn number_overflow_errors() {
        let mut out = StatData::default();
        let err = parse_stat(b"cpu  18446744073709551616 0 0 0\n", &mut out).unwrap_err();
        assert!(matches!(err, CollectError::Parse { what: "number", .. }));
        assert!(parse_stat(b"cpu  18446744073709551615 0 0 0\n", &mut out).is_ok());
    }

    #[test]
    fn cpu_index_overflow_errors() {
        let mut out = StatData::default();
        let err = parse_stat(b"cpu  1 1 1 1\ncpu65536 1 1 1 1\n", &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::Parse {
                what: "cpu index",
                ..
            }
        ));
        parse_stat(b"cpu  1 1 1 1\ncpu65535 1 1 1 1\n", &mut out).unwrap();
        assert_eq!(out.per_cpu[0].0, 65535);
    }
}
