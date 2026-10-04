//! Parser for /proc/swaps (PERF-045).
use std::sync::Arc;

use crate::CollectError;

const NOT_A_NUMBER: CollectError = CollectError::Parse {
    file: "swaps",
    what: "not a number",
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapKind {
    Partition,
    File,
    Zram,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwapEntry {
    pub name: Arc<str>,
    pub kind: SwapKind,
    pub size_bytes: u64,
    pub used_bytes: u64,
    pub priority: i32,
}

/// Clears `out`, skips the header line and pushes one entry per line with at least 5 fields.
/// Size and Used are KiB in the file.
pub fn parse_swaps(buf: &[u8], out: &mut Vec<SwapEntry>) -> Result<(), CollectError> {
    out.clear();
    for line in buf.split(|&b| b == b'\n').skip(1) {
        let mut fields = line
            .split(u8::is_ascii_whitespace)
            .filter(|w| !w.is_empty());
        let (Some(name), Some(kind), Some(size), Some(used), Some(priority)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let name = unescape(name);
        let kind = if name.starts_with(b"/dev/zram") {
            SwapKind::Zram
        } else {
            match kind {
                b"partition" => SwapKind::Partition,
                b"file" => SwapKind::File,
                _ => SwapKind::Other,
            }
        };
        out.push(SwapEntry {
            name: Arc::from(String::from_utf8_lossy(&name)),
            kind,
            size_bytes: parse::<u64>(size)?.saturating_mul(1024),
            used_bytes: parse::<u64>(used)?.saturating_mul(1024),
            priority: parse(priority)?,
        });
    }
    Ok(())
}

/// Decodes the kernel's `\ooo` octal escapes (`\040` space, `\011` tab, `\012` newline,
/// `\134` backslash). A backslash not followed by three octal digits is kept as is.
fn unescape(word: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(word.len());
    let mut i = 0;
    while i < word.len() {
        if word[i] == b'\\'
            && let Some(byte) = word.get(i + 1..i + 4).and_then(octal_byte)
        {
            out.push(byte);
            i += 4;
        } else {
            out.push(word[i]);
            i += 1;
        }
    }
    out
}

fn octal_byte(digits: &[u8]) -> Option<u8> {
    let mut value: u32 = 0;
    for &d in digits {
        if !(b'0'..=b'7').contains(&d) {
            return None;
        }
        value = value * 8 + u32::from(d - b'0');
    }
    u8::try_from(value).ok()
}

fn parse<T: std::str::FromStr>(word: &[u8]) -> Result<T, CollectError> {
    std::str::from_utf8(word)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(NOT_A_NUMBER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const HEADER: &str = "Filename\t\t\t\tType\t\tSize\t\tUsed\t\tPriority\n";

    fn parsed(text: &str) -> Vec<SwapEntry> {
        let mut out = Vec::new();
        parse_swaps(text.as_bytes(), &mut out).unwrap();
        out
    }

    #[test]
    fn parse_three_kinds() {
        let text = format!(
            "{HEADER}/dev/zram0  partition  8388604  1024  100\n\
             /swap\\040file  file  2097148  0  -2\n\
             /dev/nvme0n1p3  partition  16777212  0  -3\n"
        );
        let out = parsed(&text);
        assert_eq!(
            out,
            vec![
                SwapEntry {
                    name: Arc::from("/dev/zram0"),
                    kind: SwapKind::Zram,
                    size_bytes: 8_589_930_496,
                    used_bytes: 1_048_576,
                    priority: 100,
                },
                SwapEntry {
                    name: Arc::from("/swap file"),
                    kind: SwapKind::File,
                    size_bytes: 2_097_148 * 1024,
                    used_bytes: 0,
                    priority: -2,
                },
                SwapEntry {
                    name: Arc::from("/dev/nvme0n1p3"),
                    kind: SwapKind::Partition,
                    size_bytes: 16_777_212 * 1024,
                    used_bytes: 0,
                    priority: -3,
                },
            ]
        );
    }

    #[test]
    fn header_only() {
        assert!(parsed(HEADER).is_empty());
        assert!(parsed("").is_empty());
        // A short line is skipped.
        assert!(parsed(&format!("{HEADER}/dev/sda2 partition 100 0\n")).is_empty());
    }

    #[test]
    fn octal_escapes() {
        let text = format!("{HEADER}/a\\040b\\011c\\012d\\134e\\x  weird  1  1  0\n");
        let out = parsed(&text);
        assert_eq!(&*out[0].name, "/a b\tc\nd\\e\\x");
        assert_eq!(out[0].kind, SwapKind::Other);
        // Out of byte range is kept literally.
        assert_eq!(unescape(b"\\777"), b"\\777");
    }

    proptest! {
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=512)) {
            let mut out = Vec::new();
            let _ = parse_swaps(&bytes, &mut out);
        }

        #[test]
        fn prop_never_panics_on_text(text in "[0-9a-z/\\\\ \t\n-]{0,300}") {
            let mut out = Vec::new();
            let _ = parse_swaps(text.as_bytes(), &mut out);
        }
    }
}
