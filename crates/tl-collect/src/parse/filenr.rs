//! Parser for /proc/sys/fs/file-nr (PERF-033).

use crate::CollectError;

const FORMAT_ERROR: CollectError = CollectError::Parse {
    file: "/proc/sys/fs/file-nr",
    what: "format",
};

/// "12345\t0\t9223372036854775807\n": returns field 1 (allocated file handles, system-wide).
pub fn parse_file_nr(buf: &[u8]) -> Result<u64, CollectError> {
    let first = buf
        .split(u8::is_ascii_whitespace)
        .find(|w| !w.is_empty())
        .ok_or(FORMAT_ERROR)?;
    if !first.iter().all(u8::is_ascii_digit) {
        return Err(FORMAT_ERROR);
    }
    std::str::from_utf8(first)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(FORMAT_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_ok() {
        assert_eq!(
            parse_file_nr(b"12345\t0\t9223372036854775807\n").unwrap(),
            12345
        );
        assert_eq!(parse_file_nr(b"26143\t0\t2097152\n").unwrap(), 26143);
    }

    #[test]
    fn parse_error() {
        for text in [
            "",
            "\n",
            "x\t0\t1\n",
            "+5\t0\t1\n",
            "99999999999999999999\t0\t1\n",
        ] {
            assert!(
                matches!(
                    parse_file_nr(text.as_bytes()),
                    Err(CollectError::Parse {
                        file: "/proc/sys/fs/file-nr",
                        what: "format"
                    })
                ),
                "{text:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=256)) {
            let _ = parse_file_nr(&bytes);
        }
    }
}
