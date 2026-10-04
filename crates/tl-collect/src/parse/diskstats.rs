//! Parser for /proc/diskstats (ADR-016, R-052). One read per tick covers every disk.

use memchr::memchr;
use smallvec::SmallVec;

use crate::CollectError;

/// The numbers a kernel before 4.18 writes after `major minor name`.
const MIN_NUMBERS: usize = 11;
/// The numbers a 5.5+ kernel writes (discard fields from 4.18, flush fields from 5.5).
const MAX_NUMBERS: usize = 17;

/// One line of /proc/diskstats. Fields that the running kernel does not write are 0.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskStatLine {
    pub major: u32,
    pub minor: u32,
    pub name: SmallVec<[u8; 32]>,
    pub reads: u64,
    pub reads_merged: u64,
    pub sectors_read: u64,
    pub read_ms: u64,
    pub writes: u64,
    pub writes_merged: u64,
    pub sectors_written: u64,
    pub write_ms: u64,
    pub in_flight: u64,
    pub io_ms: u64,
    pub weighted_ms: u64,
    pub discards: u64,
    pub discards_merged: u64,
    pub sectors_discarded: u64,
    pub discard_ms: u64,
    pub flushes: u64,
    pub flush_ms: u64,
}

/// Clears `out` (keeping its capacity) and fills it with one entry per valid line of `buf`.
///
/// A line with fewer than 3 + 11 tokens or with a value that is not a number is skipped.
pub fn parse_diskstats(buf: &[u8], out: &mut Vec<DiskStatLine>) -> Result<(), CollectError> {
    out.clear();
    let mut rest = buf;
    while !rest.is_empty() {
        let (line, next) = match memchr(b'\n', rest) {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        if let Some(entry) = parse_line(line) {
            out.push(entry);
        }
    }
    Ok(())
}

fn parse_line(line: &[u8]) -> Option<DiskStatLine> {
    let mut tokens = line
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|t| !t.is_empty());
    let major = u32::try_from(parse_u64(tokens.next()?)?).ok()?;
    let minor = u32::try_from(parse_u64(tokens.next()?)?).ok()?;
    let name = SmallVec::from_slice(tokens.next()?);

    let mut n = [0u64; MAX_NUMBERS];
    let mut count = 0;
    for token in tokens {
        // Numbers past the 17th come from a newer kernel; ignore them.
        if count < MAX_NUMBERS {
            n[count] = parse_u64(token)?;
            count += 1;
        }
    }
    if count < MIN_NUMBERS {
        return None;
    }

    Some(DiskStatLine {
        major,
        minor,
        name,
        reads: n[0],
        reads_merged: n[1],
        sectors_read: n[2],
        read_ms: n[3],
        writes: n[4],
        writes_merged: n[5],
        sectors_written: n[6],
        write_ms: n[7],
        in_flight: n[8],
        io_ms: n[9],
        weighted_ms: n[10],
        discards: n[11],
        discards_merged: n[12],
        sectors_discarded: n[13],
        discard_ms: n[14],
        flushes: n[15],
        flush_ms: n[16],
    })
}

/// Decimal digits only; `None` on anything else or on overflow.
fn parse_u64(token: &[u8]) -> Option<u64> {
    if token.is_empty() {
        return None;
    }
    token.iter().try_fold(0u64, |acc, b| {
        let digit = b.checked_sub(b'0').filter(|d| *d <= 9)?;
        acc.checked_mul(10)?.checked_add(u64::from(digit))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::sample::Index;

    fn owner_desktop() -> Vec<u8> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/owner-desktop/proc/diskstats"
        );
        std::fs::read(path).unwrap()
    }

    /// The fixture cut at a random point, with up to 16 bytes flipped and up to 8 separator or
    /// digit bytes inserted.
    fn mutated() -> impl Strategy<Value = Vec<u8>> {
        (
            any::<Index>(),
            prop::collection::vec((any::<Index>(), any::<u8>()), 0..=16),
            prop::collection::vec(
                (any::<Index>(), prop::sample::select(b"\n \t0123456789x".to_vec())),
                0..=8,
            ),
        )
            .prop_map(|(cut, flips, inserts)| {
                let mut bytes = owner_desktop();
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
            let mut out = Vec::new();
            prop_assert!(parse_diskstats(&bytes, &mut out).is_ok());
            prop_assert!(out.len() <= bytes.iter().filter(|b| **b == b'\n').count() + 1);
        }
    }

    #[test]
    fn parse_u64_rejects_non_digits_and_overflow() {
        assert_eq!(parse_u64(b"0"), Some(0));
        assert_eq!(parse_u64(b"18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_u64(b"18446744073709551616"), None);
        assert_eq!(parse_u64(b"-1"), None);
        assert_eq!(parse_u64(b"1x"), None);
        assert_eq!(parse_u64(b""), None);
    }
}
