//! Parser for /proc/vmstat. Reads `pswpin` only (ADR-017); R-162 holds for every other field.

use crate::CollectError;

const FORMAT_ERROR: CollectError = CollectError::Parse {
    file: "/proc/vmstat",
    what: "pswpin",
};

/// The `pswpin` counter: pages swapped in since boot. `None` when the line is absent (a
/// kernel built without VM event counters).
pub fn parse_pswpin(buf: &[u8]) -> Result<Option<u64>, CollectError> {
    for line in buf.split(|&b| b == b'\n') {
        let Some(value) = line.strip_prefix(b"pswpin ") else {
            continue;
        };
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(FORMAT_ERROR);
        }
        return std::str::from_utf8(value)
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Some)
            .ok_or(FORMAT_ERROR);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_ok() {
        let text = b"nr_free_pages 3179272\npgpgout 0\npswpin 188164513\npswpout 205881221\n";
        assert_eq!(parse_pswpin(text).unwrap(), Some(188_164_513));
        // `pswpout` and `pswpin_x` do not match.
        assert_eq!(parse_pswpin(b"pswpout 5\npswpinx 7\n").unwrap(), None);
        assert_eq!(parse_pswpin(b"").unwrap(), None);
        assert_eq!(parse_pswpin(b"pswpin 0").unwrap(), Some(0));
    }

    #[test]
    fn parse_errors() {
        for text in [
            "pswpin \n",
            "pswpin x\n",
            "pswpin -1\n",
            "pswpin 99999999999999999999999\n",
        ] {
            assert!(
                matches!(
                    parse_pswpin(text.as_bytes()),
                    Err(CollectError::Parse {
                        file: "/proc/vmstat",
                        what: "pswpin"
                    })
                ),
                "{text:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=256)) {
            let _ = parse_pswpin(&bytes);
        }

        #[test]
        fn prop_round_trip(n: u64) {
            let text = format!("pgfault 1\npswpin {n}\npswpout 2\n");
            prop_assert_eq!(parse_pswpin(text.as_bytes()).unwrap(), Some(n));
        }
    }
}
