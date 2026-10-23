//! Parser for /proc/pressure/{cpu,memory} (ADR-016 "System pressure").

use crate::CollectError;

const FORMAT_ERROR: CollectError = CollectError::Parse {
    file: "/proc/pressure",
    what: "format",
};

/// The avg10 values of one pressure file, in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PsiAvg10 {
    pub some: f32,
    /// `None` when the file has no `full` line (/proc/pressure/cpu before Linux 5.13).
    pub full: Option<f32>,
}

/// "some avg10=0.12 avg60=0.05 avg300=0.01 total=123\nfull avg10=...\n". The `some` line is
/// required; other lines are ignored.
pub fn parse_psi(buf: &[u8]) -> Result<PsiAvg10, CollectError> {
    let mut some = None;
    let mut full = None;
    for line in buf.split(|&b| b == b'\n') {
        let mut words = line.split(|&b| b == b' ').filter(|w| !w.is_empty());
        let slot = match words.next() {
            Some(b"some") => &mut some,
            Some(b"full") => &mut full,
            _ => continue,
        };
        let avg10 = words
            .find_map(|w| w.strip_prefix(b"avg10="))
            .ok_or(FORMAT_ERROR)?;
        *slot = Some(percent(avg10)?);
    }
    Ok(PsiAvg10 {
        some: some.ok_or(FORMAT_ERROR)?,
        full,
    })
}

/// A finite decimal from 0 to 100 such as "1.25".
fn percent(word: &[u8]) -> Result<f32, CollectError> {
    if word.is_empty() || !word.iter().all(|&b| b.is_ascii_digit() || b == b'.') {
        return Err(FORMAT_ERROR);
    }
    let value: f32 = std::str::from_utf8(word)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(FORMAT_ERROR)?;
    if value.is_finite() && value <= 100.0 {
        Ok(value)
    } else {
        Err(FORMAT_ERROR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_memory() {
        let text = b"some avg10=1.25 avg60=0.40 avg300=0.10 total=123456\n\
                     full avg10=0.50 avg60=0.20 avg300=0.05 total=65432\n";
        assert_eq!(
            parse_psi(text).unwrap(),
            PsiAvg10 {
                some: 1.25,
                full: Some(0.5),
            }
        );
    }

    #[test]
    fn parse_cpu_without_full() {
        let text = b"some avg10=12.00 avg60=8.00 avg300=4.00 total=99\n";
        assert_eq!(
            parse_psi(text).unwrap(),
            PsiAvg10 {
                some: 12.0,
                full: None,
            }
        );
    }

    #[test]
    fn parse_errors() {
        for text in [
            "",
            "full avg10=0.50 avg60=0.20 avg300=0.05 total=1\n",
            "some avg60=0.40 avg300=0.10 total=1\n",
            "some avg10= avg60=0.40\n",
            "some avg10=x avg60=0.40\n",
            "some avg10=-1.00 avg60=0.40\n",
            "some avg10=101.00 avg60=0.40\n",
            "some avg10=1.00\nfull avg10=y\n",
        ] {
            assert!(
                matches!(
                    parse_psi(text.as_bytes()),
                    Err(CollectError::Parse {
                        file: "/proc/pressure",
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
            let _ = parse_psi(&bytes);
        }

        #[test]
        fn prop_never_panics_on_text(text in "(some|full|avg10=|[0-9. =\n])*") {
            let _ = parse_psi(text.as_bytes());
        }

        #[test]
        fn prop_round_trip(some in 0u32..=10_000, full in 0u32..=10_000) {
            let text = format!(
                "some avg10={}.{:02} avg60=0.00 avg300=0.00 total=1\n\
                 full avg10={}.{:02} avg60=0.00 avg300=0.00 total=2\n",
                some / 100, some % 100, full / 100, full % 100
            );
            let got = parse_psi(text.as_bytes()).unwrap();
            prop_assert!((got.some - some as f32 / 100.0).abs() < 1e-4);
            prop_assert!((got.full.unwrap() - full as f32 / 100.0).abs() < 1e-4);
        }
    }
}
