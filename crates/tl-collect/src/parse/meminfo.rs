//! Parser for /proc/meminfo (ADR-016, ADR-008, PERF-041 to PERF-044).

use crate::CollectError;

const FILE: &str = "/proc/meminfo";

/// Values in bytes (meminfo "kB" × 1024) except the two HugePages counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemInfo {
    pub mem_total: u64,
    pub mem_free: u64,
    pub mem_available: u64,
    pub buffers: u64,
    pub cached: u64,
    pub swap_cached: u64,
    pub dirty: u64,
    pub writeback: u64,
    pub shmem: u64,
    pub sreclaimable: u64,
    pub sunreclaim: u64,
    pub committed_as: u64,
    pub commit_limit: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub zswap: Option<u64>,
    pub zswapped: Option<u64>,
    pub hugepages_total: Option<u64>,
    pub hugepages_free: Option<u64>,
    pub hugepagesize: Option<u64>,
}

/// Number of required keys; each sets one bit of the `seen` mask.
const REQUIRED: u32 = 15;

/// Parses `buf` in one pass, with no allocation. On success `out` is fully overwritten; on error
/// its contents are unspecified.
pub fn parse_meminfo(buf: &[u8], out: &mut MemInfo) -> Result<(), CollectError> {
    *out = MemInfo::default();
    let mut seen: u32 = 0;
    for line in buf.split(|&b| b == b'\n') {
        let Some(colon) = memchr::memchr(b':', line) else {
            continue;
        };
        let key = &line[..colon];
        let rest = &line[colon + 1..];
        // (bit in `seen` for a required key, field, scale by 1024)
        let (bit, slot, scaled): (Option<u32>, Slot<'_>, bool) = match key {
            b"MemTotal" => (Some(0), Slot::Req(&mut out.mem_total), true),
            b"MemFree" => (Some(1), Slot::Req(&mut out.mem_free), true),
            b"MemAvailable" => (Some(2), Slot::Req(&mut out.mem_available), true),
            b"Buffers" => (Some(3), Slot::Req(&mut out.buffers), true),
            b"Cached" => (Some(4), Slot::Req(&mut out.cached), true),
            b"SwapCached" => (Some(5), Slot::Req(&mut out.swap_cached), true),
            b"Dirty" => (Some(6), Slot::Req(&mut out.dirty), true),
            b"Writeback" => (Some(7), Slot::Req(&mut out.writeback), true),
            b"Shmem" => (Some(8), Slot::Req(&mut out.shmem), true),
            b"SReclaimable" => (Some(9), Slot::Req(&mut out.sreclaimable), true),
            b"SUnreclaim" => (Some(10), Slot::Req(&mut out.sunreclaim), true),
            b"Committed_AS" => (Some(11), Slot::Req(&mut out.committed_as), true),
            b"CommitLimit" => (Some(12), Slot::Req(&mut out.commit_limit), true),
            b"SwapTotal" => (Some(13), Slot::Req(&mut out.swap_total), true),
            b"SwapFree" => (Some(14), Slot::Req(&mut out.swap_free), true),
            b"Zswap" => (None, Slot::Opt(&mut out.zswap), true),
            b"Zswapped" => (None, Slot::Opt(&mut out.zswapped), true),
            b"HugePages_Total" => (None, Slot::Opt(&mut out.hugepages_total), false),
            b"HugePages_Free" => (None, Slot::Opt(&mut out.hugepages_free), false),
            b"Hugepagesize" => (None, Slot::Opt(&mut out.hugepagesize), true),
            _ => continue,
        };
        let mut value = parse_number(rest)?;
        if scaled {
            value = value.checked_mul(1024).ok_or(number_error())?;
        }
        match slot {
            Slot::Req(field) => *field = value,
            Slot::Opt(field) => *field = Some(value),
        }
        if let Some(bit) = bit {
            seen |= 1 << bit;
        }
    }
    if seen != (1 << REQUIRED) - 1 {
        return Err(CollectError::Parse {
            file: FILE,
            what: "missing field",
        });
    }
    Ok(())
}

enum Slot<'a> {
    Req(&'a mut u64),
    Opt(&'a mut Option<u64>),
}

fn number_error() -> CollectError {
    CollectError::Parse {
        file: FILE,
        what: "number",
    }
}

/// The first whitespace-separated token after the colon as a decimal `u64`. Any unit after it is
/// not checked.
fn parse_number(rest: &[u8]) -> Result<u64, CollectError> {
    let token = rest
        .split(|b| b.is_ascii_whitespace())
        .find(|t| !t.is_empty())
        .ok_or(number_error())?;
    let mut value: u64 = 0;
    for &b in token {
        if !b.is_ascii_digit() {
            return Err(number_error());
        }
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(b - b'0')))
            .ok_or(number_error())?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::sample::Index;

    const OWNER_DESKTOP: &[u8] = include_bytes!("../../tests/fixtures/owner-desktop/proc/meminfo");

    /// Cut the fixture, flip some bytes and insert separators or digits.
    fn mutated() -> impl Strategy<Value = Vec<u8>> {
        (
            any::<Index>(),
            prop::collection::vec((any::<Index>(), any::<u8>()), 0..=16),
            prop::collection::vec(
                (
                    any::<Index>(),
                    prop::sample::select(b"\n :0123456789".as_slice()),
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
        fn prop_never_panics(
            bytes in prop_oneof![prop::collection::vec(any::<u8>(), 0..=8192), mutated()]
        ) {
            let mut out = MemInfo::default();
            let _ = parse_meminfo(&bytes, &mut out);
        }
    }
}
