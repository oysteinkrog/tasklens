//! Parser for /proc/net/dev (ADR-016, R-052). One read per tick covers every interface.
//! Download is rx, upload is tx.

use memchr::memchr;
use smallvec::SmallVec;

use crate::CollectError;

/// Numbers after `name:`: 8 receive counters, then 8 transmit counters.
const NUMBERS: usize = 16;
/// Header lines at the top of the file.
const HEADER_LINES: usize = 2;

/// One interface line of /proc/net/dev.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetDevLine {
    pub name: SmallVec<[u8; 16]>,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_drop: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_drop: u64,
}

/// Clears `out` (keeping its capacity) and fills it with one entry per valid line of `buf`.
///
/// The first two lines are headers. A line with no `:`, an empty name, fewer than 16 numbers or
/// a value that is not a number is skipped.
pub fn parse_net_dev(buf: &[u8], out: &mut Vec<NetDevLine>) -> Result<(), CollectError> {
    out.clear();
    let mut rest = buf;
    let mut index = 0;
    while !rest.is_empty() {
        let (line, next) = match memchr(b'\n', rest) {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        index += 1;
        if index <= HEADER_LINES {
            continue;
        }
        if let Some(entry) = parse_line(line) {
            out.push(entry);
        }
    }
    Ok(())
}

fn parse_line(line: &[u8]) -> Option<NetDevLine> {
    let colon = memchr(b':', line)?;
    let name = trim_start(&line[..colon]);
    if name.is_empty() {
        return None;
    }

    let mut n = [0u64; NUMBERS];
    let mut count = 0;
    for token in line[colon + 1..]
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|t| !t.is_empty())
    {
        // Numbers past the 16th would come from a newer kernel; ignore them.
        if count < NUMBERS {
            n[count] = parse_u64(token)?;
            count += 1;
        }
    }
    if count < NUMBERS {
        return None;
    }

    Some(NetDevLine {
        name: SmallVec::from_slice(name),
        rx_bytes: n[0],
        rx_packets: n[1],
        rx_errors: n[2],
        rx_drop: n[3],
        tx_bytes: n[8],
        tx_packets: n[9],
        tx_errors: n[10],
        tx_drop: n[11],
    })
}

fn trim_start(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| *b != b' ' && *b != b'\t')
        .unwrap_or(bytes.len());
    &bytes[start..]
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
            "/tests/fixtures/owner-desktop/proc/net/dev"
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
                (
                    any::<Index>(),
                    prop::sample::select(b"\n \t:0123456789x".to_vec()),
                ),
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
            prop_assert!(parse_net_dev(&bytes, &mut out).is_ok());
            prop_assert!(out.len() <= bytes.iter().filter(|b| **b == b'\n').count() + 1);
            prop_assert!(out.iter().all(|l| !l.name.is_empty()));
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
