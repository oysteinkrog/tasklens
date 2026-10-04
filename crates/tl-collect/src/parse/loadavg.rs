//! Parser for /proc/loadavg (PERF-033).

use crate::CollectError;

const FORMAT_ERROR: CollectError = CollectError::Parse {
    file: "/proc/loadavg",
    what: "format",
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadAvg {
    pub one: f32,
    pub five: f32,
    pub fifteen: f32,
    pub running: u32,
    pub total: u32,
}

/// "0.52 0.58 0.59 2/1523 12345\n": three averages, then running/total scheduling entities.
pub fn parse_loadavg(buf: &[u8]) -> Result<LoadAvg, CollectError> {
    let mut fields = buf
        .split(u8::is_ascii_whitespace)
        .filter(|w| !w.is_empty());
    let (Some(one), Some(five), Some(fifteen), Some(entities)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(FORMAT_ERROR);
    };
    let slash = entities
        .iter()
        .position(|&b| b == b'/')
        .ok_or(FORMAT_ERROR)?;
    let (running, total) = (&entities[..slash], &entities[slash + 1..]);
    Ok(LoadAvg {
        one: average(one)?,
        five: average(five)?,
        fifteen: average(fifteen)?,
        running: number(running)?,
        total: number(total)?,
    })
}

/// A finite, non-negative decimal such as "0.52".
fn average(word: &[u8]) -> Result<f32, CollectError> {
    if !word.iter().all(|&b| b.is_ascii_digit() || b == b'.') {
        return Err(FORMAT_ERROR);
    }
    let value: f32 = number(word)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(FORMAT_ERROR)
    }
}

fn number<T: std::str::FromStr>(word: &[u8]) -> Result<T, CollectError> {
    std::str::from_utf8(word)
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
            parse_loadavg(b"0.52 0.58 0.59 2/1523 12345\n").unwrap(),
            LoadAvg {
                one: 0.52,
                five: 0.58,
                fifteen: 0.59,
                running: 2,
                total: 1523,
            }
        );
        // The pid field is optional for this parser.
        assert_eq!(
            parse_loadavg(b"31.35 27.73 27.82 19/6667").unwrap(),
            LoadAvg {
                one: 31.35,
                five: 27.73,
                fifteen: 27.82,
                running: 19,
                total: 6667,
            }
        );
    }

    #[test]
    fn parse_errors() {
        for text in [
            "0.52 0.58\n",
            "0.52 0.58 0.59 21523 1\n",
            "",
            "0.52 0.58 x 2/1523 1\n",
            "0.52 0.58 0.59 2/ 1\n",
            "0.52 0.58 0.59 /1523 1\n",
            "inf 0.58 0.59 2/1523 1\n",
            "-1 0.58 0.59 2/1523 1\n",
        ] {
            assert!(
                matches!(
                    parse_loadavg(text.as_bytes()),
                    Err(CollectError::Parse {
                        file: "/proc/loadavg",
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
            let _ = parse_loadavg(&bytes);
        }

        #[test]
        fn prop_never_panics_on_text(text in "[0-9./ \n-]{0,64}") {
            let _ = parse_loadavg(text.as_bytes());
        }
    }
}
