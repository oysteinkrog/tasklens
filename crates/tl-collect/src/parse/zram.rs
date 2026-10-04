//! zram devices under /sys/block/zram<N>: `disksize`, `mm_stat`, `comp_algorithm` and the
//! sector counters in `stat` (PERF-042 to PERF-044, PROC-012, ADR-016).
use std::path::PathBuf;
use std::sync::Arc;

use crate::sysroot::{KeptFile, SMALL_FILE_LIMIT, read_file, read_u64};
use crate::{CollectError, SysRoot};

const MM_STAT_FILE: &str = "zram mm_stat";
const BLOCK_STAT_FILE: &str = "block stat";
const TOO_FEW_FIELDS: &str = "too few fields";
const NOT_A_NUMBER: &str = "not a number";

/// The fields of `mm_stat`, in file order. Fields an older kernel does not write are 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZramMmStat {
    pub orig_data_size: u64,
    pub compr_data_size: u64,
    pub mem_used_total: u64,
    pub mem_limit: u64,
    pub mem_used_max: u64,
    pub same_pages: u64,
    pub pages_compacted: u64,
    pub huge_pages: u64,
}

/// Parses `mm_stat`. At least 3 fields are required; missing later fields are 0 and extra
/// fields are ignored.
pub fn parse_mm_stat(buf: &[u8], out: &mut ZramMmStat) -> Result<(), CollectError> {
    let mut fields = [0u64; 8];
    let mut count = 0;
    for (slot, word) in fields.iter_mut().zip(words(buf)) {
        *slot = parse_u64(word, MM_STAT_FILE)?;
        count += 1;
    }
    if count < 3 {
        return Err(CollectError::Parse {
            file: MM_STAT_FILE,
            what: TOO_FEW_FIELDS,
        });
    }
    let [
        orig_data_size,
        compr_data_size,
        mem_used_total,
        mem_limit,
        mem_used_max,
        same_pages,
        pages_compacted,
        huge_pages,
    ] = fields;
    *out = ZramMmStat {
        orig_data_size,
        compr_data_size,
        mem_used_total,
        mem_limit,
        mem_used_max,
        same_pages,
        pages_compacted,
        huge_pages,
    };
    Ok(())
}

/// "lzo lzo-rle [zstd] lz4\n" -> Some(b"zstd"); no brackets -> None.
pub fn parse_comp_algorithm(buf: &[u8]) -> Option<&[u8]> {
    let open = memchr::memchr(b'[', buf)?;
    let rest = &buf[open + 1..];
    let close = memchr::memchr(b']', rest)?;
    let name = &rest[..close];
    (!name.is_empty()).then_some(name)
}

/// /sys/block/<d>/stat: returns (sectors read = field 3, sectors written = field 7), 1-based.
pub fn parse_block_stat_sectors(buf: &[u8]) -> Result<(u64, u64), CollectError> {
    let mut fields = words(buf);
    let too_few = || CollectError::Parse {
        file: BLOCK_STAT_FILE,
        what: TOO_FEW_FIELDS,
    };
    let read = fields.nth(2).ok_or_else(too_few)?;
    let written = fields.nth(3).ok_or_else(too_few)?;
    Ok((
        parse_u64(read, BLOCK_STAT_FILE)?,
        parse_u64(written, BLOCK_STAT_FILE)?,
    ))
}

/// One zram device. `disksize_bytes` 0 means the device is not set up.
pub struct ZramDevice {
    /// For example "zram0".
    pub name: Arc<str>,
    pub disksize_bytes: u64,
    pub algorithm: Option<Arc<str>>,
    /// Kept open (ADR-016).
    mm_stat: KeptFile,
    /// Read one-shot per sample.
    stat_path: PathBuf,
}

impl ZramDevice {
    pub fn read_mm_stat(
        &mut self,
        buf: &mut Vec<u8>,
        out: &mut ZramMmStat,
    ) -> Result<(), CollectError> {
        self.mm_stat.read(buf)?;
        parse_mm_stat(buf, out)
    }

    /// (sectors read, sectors written) from `stat`.
    pub fn read_sectors(&self, buf: &mut Vec<u8>) -> Result<(u64, u64), CollectError> {
        read_file(&self.stat_path, buf, SMALL_FILE_LIMIT)?;
        parse_block_stat_sectors(buf)
    }
}

/// Every root.sys/block/zram<N>, sorted by N, with disksize and algorithm read once.
///
/// Devices with `disksize` 0 are included. Clears `out` first.
pub fn enumerate(root: &SysRoot, out: &mut Vec<ZramDevice>) -> Result<(), CollectError> {
    out.clear();
    let block = root.sys_path("block");
    let mut found: Vec<(u32, String)> = Vec::new();
    for entry in std::fs::read_dir(&block)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some(n) = zram_index(&name) {
            found.push((n, name));
        }
    }
    found.sort_unstable_by_key(|(n, _)| *n);

    let mut buf = Vec::new();
    for (_, name) in found {
        let dir = block.join(&name);
        let disksize_bytes = read_u64(&dir.join("disksize"), &mut buf)?;
        read_file(&dir.join("comp_algorithm"), &mut buf, SMALL_FILE_LIMIT)?;
        let algorithm = parse_comp_algorithm(&buf)
            .and_then(|a| std::str::from_utf8(a).ok())
            .map(Arc::from);
        out.push(ZramDevice {
            name: Arc::from(name),
            disksize_bytes,
            algorithm,
            mm_stat: KeptFile::new(dir.join("mm_stat")),
            stat_path: dir.join("stat"),
        });
    }
    Ok(())
}

/// "zram12" -> Some(12). Anything else -> None.
fn zram_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("zram")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn words(buf: &[u8]) -> impl Iterator<Item = &[u8]> {
    buf.split(u8::is_ascii_whitespace).filter(|w| !w.is_empty())
}

fn parse_u64(word: &[u8], file: &'static str) -> Result<u64, CollectError> {
    std::str::from_utf8(word)
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(CollectError::Parse {
            file,
            what: NOT_A_NUMBER,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn mm_stat_extra_fields_ignored() {
        let mut out = ZramMmStat::default();
        parse_mm_stat(b"1 2 3 4 5 6 7 8 9 10\n", &mut out).unwrap();
        assert_eq!(out.huge_pages, 8);
    }

    #[test]
    fn mm_stat_bad_number() {
        let mut out = ZramMmStat::default();
        assert!(matches!(
            parse_mm_stat(b"1 x 3\n", &mut out),
            Err(CollectError::Parse { .. })
        ));
    }

    #[test]
    fn block_stat_too_few() {
        assert!(matches!(
            parse_block_stat_sectors(b"1 2 3 4 5 6\n"),
            Err(CollectError::Parse { .. })
        ));
        assert_eq!(
            parse_block_stat_sectors(b"1 2 3 4 5 6 7\n").unwrap(),
            (3, 7)
        );
    }

    #[test]
    fn comp_algorithm_edges() {
        assert_eq!(parse_comp_algorithm(b"[lzo-rle] zstd\n"), Some(&b"lzo-rle"[..]));
        assert_eq!(parse_comp_algorithm(b"lzo [] zstd\n"), None);
        assert_eq!(parse_comp_algorithm(b"lzo [zstd\n"), None);
    }

    #[test]
    fn zram_index_names() {
        assert_eq!(zram_index("zram0"), Some(0));
        assert_eq!(zram_index("zram12"), Some(12));
        assert_eq!(zram_index("zram"), None);
        assert_eq!(zram_index("zram+1"), None);
        assert_eq!(zram_index("nvme0n1"), None);
    }

    proptest! {
        #[test]
        fn prop_parsers_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..=512)) {
            let mut out = ZramMmStat::default();
            let _ = parse_mm_stat(&bytes, &mut out);
            let _ = parse_comp_algorithm(&bytes);
            let _ = parse_block_stat_sectors(&bytes);
        }

        #[test]
        fn prop_parsers_never_panic_on_numbers(text in "[0-9 \t\n\\[\\]a-z-]{0,200}") {
            let mut out = ZramMmStat::default();
            let _ = parse_mm_stat(text.as_bytes(), &mut out);
            let _ = parse_comp_algorithm(text.as_bytes());
            let _ = parse_block_stat_sectors(text.as_bytes());
        }
    }
}
