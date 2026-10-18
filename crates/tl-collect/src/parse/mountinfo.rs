//! Parser for /proc/self/mountinfo (PERF-049).
use std::sync::Arc;

use crate::CollectError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    pub major: u32,
    pub minor: u32,
    pub mount_point: Arc<str>,
    pub fs_type: Arc<str>,
    /// The block device name behind a `/dev/` mount source, with any btrfs `[/subvol]` suffix
    /// removed (`nvme0n1p2` for `/dev/nvme0n1p2[/@home]`). `None` for other sources.
    ///
    /// A btrfs subvolume mount reports an anonymous `0:N` device number, so the name is the only
    /// link back to its partition.
    pub source_dev: Option<Arc<str>>,
}

impl MountEntry {
    /// The mount sits on the block device `name` (a disk, partition or dm node), by its device
    /// number or, for anonymous device numbers, by its mount source.
    pub fn is_on(&self, major: u32, minor: u32, name: &str) -> bool {
        if self.major != 0 {
            return (self.major, self.minor) == (major, minor);
        }
        self.source_dev.as_deref() == Some(name)
    }
}

/// Clears `out` and pushes one entry per line of the form
/// `id parent major:minor root mount_point options [optional fields...] - fs_type source super_options`.
/// Lines that do not match are skipped.
pub fn parse_mountinfo(buf: &[u8], out: &mut Vec<MountEntry>) -> Result<(), CollectError> {
    out.clear();
    for line in buf.split(|&b| b == b'\n') {
        if let Some(entry) = parse_line(line) {
            out.push(entry);
        }
    }
    Ok(())
}

fn parse_line(line: &[u8]) -> Option<MountEntry> {
    let mut fields = line.split(|&b| b == b' ').filter(|w| !w.is_empty());
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let dev = fields.next()?;
    let _root = fields.next()?;
    let mount_point = fields.next()?;
    let _options = fields.next()?;
    fields.by_ref().find(|w| *w == b"-")?;
    let fs_type = fields.next()?;
    let source = fields.next().map(unescape);

    let colon = dev.iter().position(|&b| b == b':')?;
    let major = parse_u32(&dev[..colon])?;
    let minor = parse_u32(&dev[colon + 1..])?;
    Some(MountEntry {
        major,
        minor,
        mount_point: Arc::from(String::from_utf8_lossy(&unescape(mount_point))),
        fs_type: Arc::from(String::from_utf8_lossy(&unescape(fs_type))),
        source_dev: source.and_then(|s| source_dev(&s)),
    })
}

fn source_dev(source: &[u8]) -> Option<Arc<str>> {
    let source = String::from_utf8_lossy(source);
    let path = source.strip_prefix("/dev/")?;
    let name = path.split('[').next().unwrap_or(path);
    (!name.is_empty()).then(|| Arc::from(name))
}

fn parse_u32(word: &[u8]) -> Option<u32> {
    std::str::from_utf8(word).ok()?.parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

    fn parsed(buf: &[u8]) -> Vec<MountEntry> {
        let mut out = Vec::new();
        parse_mountinfo(buf, &mut out).unwrap();
        out
    }

    #[test]
    fn parse_fixture_lines() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/disk-partitions-dm/proc/self/mountinfo");
        let out = parsed(&std::fs::read(path).unwrap());
        let summary: Vec<_> = out
            .iter()
            .map(|e| (e.major, e.minor, &*e.mount_point, &*e.fs_type))
            .collect();
        assert_eq!(
            summary,
            vec![
                (254, 0, "/", "ext4"),
                (8, 1, "/boot", "vfat"),
                (8, 1, "/mnt/boot copy", "vfat"),
                (0, 25, "/run", "tmpfs"),
            ]
        );
    }

    #[test]
    fn btrfs_subvolume_mounts_name_their_partition() {
        let text = b"30 1 0:30 /@ / rw - btrfs /dev/nvme0n1p2[/@] rw\n\
            31 30 0:31 /@home /home rw - btrfs /dev/nvme0n1p2[/@home] rw\n\
            32 30 0:25 / /run rw - tmpfs tmpfs rw\n";
        let out = parsed(text);
        assert_eq!(out.len(), 3);
        assert!(out[0].is_on(259, 2, "nvme0n1p2"));
        assert!(out[1].is_on(259, 2, "nvme0n1p2"));
        assert!(!out[1].is_on(259, 3, "nvme0n1p3"));
        assert_eq!(out[2].source_dev, None);
        assert!(!out[2].is_on(0, 25, "tmpfs"));
    }

    #[test]
    fn octal_escape() {
        let line = b"30 22 8:3 / /a\\040b\\011c\\012d\\134e\\x rw - ext4 /dev/sda3 rw\n";
        let out = parsed(line);
        assert_eq!(&*out[0].mount_point, "/a b\tc\nd\\e\\x");
        assert_eq!(unescape(b"\\777"), b"\\777");
    }

    #[test]
    fn skips_bad_lines() {
        let text = b"\n\
            garbage\n\
            1 2 x:y / /a rw - ext4 src rw\n\
            1 2 8:1 / /b rw shared:1\n\
            1 2 8:1 / /c rw shared:1 master:2 - xfs src rw\n";
        let out = parsed(text);
        assert_eq!(out.len(), 1);
        assert_eq!(&*out[0].mount_point, "/c");
        assert_eq!(&*out[0].fs_type, "xfs");
    }

    proptest! {
        #[test]
        fn prop_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=512)) {
            let mut out = Vec::new();
            let _ = parse_mountinfo(&bytes, &mut out);
        }

        #[test]
        fn prop_never_panics_on_text(text in "[0-9a-z:/\\\\ \n-]{0,300}") {
            let mut out = Vec::new();
            let _ = parse_mountinfo(text.as_bytes(), &mut out);
        }
    }
}
