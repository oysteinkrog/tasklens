//! Parser for /proc/<pid>/statm (PROC-002, PROC-078).
//!
//! One line of seven page counts: size, resident, shared, text, lib, data, dt. `lib` (5) and
//! `dt` (7) are always 0 since Linux 2.6 and are not kept.

use crate::CollectError;

const FILE: &str = "statm";

const PARSE_ERROR: CollectError = CollectError::ProcParse { pid: 0, file: FILE };

/// The statm fields that tasklens uses, in pages. Numbers in the comments are statm fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PidStatm {
    /// 1
    pub size: u64,
    /// 2
    pub resident: u64,
    /// 3
    pub shared: u64,
    /// 4
    pub text: u64,
    /// 6
    pub data: u64,
}

/// Parses one statm line into `out`. Fewer than 7 fields, or a non-numeric field among them,
/// is `ProcParse { pid: 0, file: "statm" }`.
pub fn parse_statm(buf: &[u8], out: &mut PidStatm) -> Result<(), CollectError> {
    let mut fields = buf.trim_ascii_end().split(|&b| b == b' ');
    let mut values = [0u64; 7];
    for value in &mut values {
        *value = fields.next().and_then(parse_u64).ok_or(PARSE_ERROR)?;
    }
    let [size, resident, shared, text, _lib, data, _dt] = values;
    *out = PidStatm {
        size,
        resident,
        shared,
        text,
        data,
    };
    Ok(())
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
    fn six_fields_is_error() {
        let mut out = PidStatm::default();
        let err = parse_statm(b"1 2 3 4 0 6\n", &mut out).unwrap_err();
        assert!(matches!(
            err,
            CollectError::ProcParse {
                pid: 0,
                file: "statm"
            }
        ));
    }

    #[test]
    fn seven_fields_parsed() {
        let mut out = PidStatm::default();
        parse_statm(b"10 20 30 40 0 60 0\n", &mut out).unwrap();
        assert_eq!(
            out,
            PidStatm {
                size: 10,
                resident: 20,
                shared: 30,
                text: 40,
                data: 60
            }
        );
    }
}
