//! Block device enumeration under /sys/block/<dev>/: which disks get a page, their kind, size
//! and model, and their partitions and device-mapper children (ADR-016, PERF-047, R-052, R-140).
//!
//! Runs at start and every 5 s while disks are demanded (PERF-005), not per tick, so allocation
//! is fine here. Disk serials and WWN are not read (R-155 item 7).
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use crate::parse::udev::{UdevProps, block_entry, read_udev_props};
use crate::sanitize::sanitize_text;
use crate::sysroot::{read_trimmed, read_u64};
use crate::{CollectError, SysRoot};

const DEV_ERROR: CollectError = CollectError::Parse {
    file: "block dev",
    what: "major:minor",
};

/// Bytes of a model or dm name kept before sanitizing cuts it.
const NAME_MAX: usize = 256;

/// The udev keys this module reads (R-140 item 3).
const UDEV_KEYS: [&str; 2] = ["ID_BUS", "ID_MODEL"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskKind {
    Nvme,
    Ssd,
    Hdd,
    Emmc,
    Sd,
    Usb,
    Optical,
    Virtual,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildKind {
    Partition { number: u32, start_sector: u64 },
    DeviceMapper { dm_name: Option<Arc<str>> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockChild {
    pub name: Arc<str>,
    pub major: u32,
    pub minor: u32,
    pub size_bytes: u64,
    pub kind: ChildKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDevice {
    pub name: Arc<str>,
    pub major: u32,
    pub minor: u32,
    pub size_bytes: u64,
    pub kind: DiskKind,
    pub model: Option<Arc<str>>,
    pub removable: bool,
    /// Partitions by number, then dm devices by name.
    pub children: Vec<BlockChild>,
}

/// Names under /sys/block that never get a disk page (ADR-016, PERF-047).
pub const EXCLUDED_PREFIXES: [&str; 7] = ["loop", "ram", "zram", "fd", "dm-", "md", "zd"];

/// Every disk under root.sys/block, sorted by name, with its children. Clears `out` first.
///
/// Fails only if root.sys/block cannot be listed. A field that cannot be read keeps its
/// fallback: 0 for numbers, `false`, `None`, or the last kind rule. A name that is not UTF-8 is
/// left out.
pub fn enumerate_disks(root: &SysRoot, out: &mut Vec<BlockDevice>) -> Result<(), CollectError> {
    out.clear();
    let block = root.sys_path("block");
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&block)? {
        if let Ok(name) = entry?.file_name().into_string() {
            names.push(name);
        }
    }
    names.sort_unstable();

    let mut buf = Vec::new();
    let mut props = UdevProps::default();
    let mut text = String::new();
    for name in names
        .iter()
        .filter(|n| !EXCLUDED_PREFIXES.iter().any(|p| n.starts_with(p)))
    {
        let dir = block.join(name);
        let (major, minor) = read_dev(&dir, &mut buf).unwrap_or((0, 0));
        let path = root.run_path(&format!("udev/data/{}", block_entry(major, minor)));
        if read_udev_props(&path, &UDEV_KEYS, &mut props).is_err() {
            props.clear();
        }
        let kind = classify(root, name, &dir, &props, &mut buf);
        let model = read_model(&dir, &props, &mut buf, &mut text);
        let removable = matches!(read_trimmed(&dir.join("removable"), &mut buf), Ok(b"1"));
        out.push(BlockDevice {
            name: Arc::from(name.as_str()),
            major,
            minor,
            size_bytes: read_size(&dir, &mut buf),
            kind,
            model,
            removable,
            children: partitions(&dir, &mut buf),
        });
    }

    for name in names.iter().filter(|n| n.starts_with("dm-")) {
        let dir = block.join(name);
        let Some(slave) = first_slave(&dir) else {
            continue;
        };
        let Some(disk) = out.iter_mut().find(|d| {
            *d.name == *slave
                || d.children
                    .iter()
                    .any(|c| matches!(c.kind, ChildKind::Partition { .. }) && *c.name == *slave)
        }) else {
            continue;
        };
        let (major, minor) = read_dev(&dir, &mut buf).unwrap_or((0, 0));
        let dm_name = match read_trimmed(&dir.join("dm/name"), &mut buf) {
            Ok(raw) => clean(raw, &mut text),
            Err(_) => None,
        };
        disk.children.push(BlockChild {
            name: Arc::from(name.as_str()),
            major,
            minor,
            size_bytes: read_size(&dir, &mut buf),
            kind: ChildKind::DeviceMapper { dm_name },
        });
    }
    Ok(())
}

/// "259:0\n" -> (259, 0)
pub fn parse_dev(buf: &[u8]) -> Result<(u32, u32), CollectError> {
    let text = std::str::from_utf8(buf.trim_ascii()).map_err(|_| DEV_ERROR)?;
    let (major, minor) = text.split_once(':').ok_or(DEV_ERROR)?;
    let major = major.parse().map_err(|_| DEV_ERROR)?;
    let minor = minor.parse().map_err(|_| DEV_ERROR)?;
    Ok((major, minor))
}

/// The first PERF-047 kind rule that matches.
fn classify(
    root: &SysRoot,
    name: &str,
    dir: &Path,
    props: &UdevProps,
    buf: &mut Vec<u8>,
) -> DiskKind {
    if name.starts_with("nvme") {
        return DiskKind::Nvme;
    }
    if name.starts_with("mmcblk") {
        return match read_trimmed(&dir.join("device/type"), buf) {
            Ok(b"SD") => DiskKind::Sd,
            _ => DiskKind::Emmc,
        };
    }
    let usb = match props.get("ID_BUS") {
        Some(bus) => bus == "usb",
        None => canonical_has_component(root, dir, b"usb"),
    };
    if usb {
        return DiskKind::Usb;
    }
    if name.starts_with("sr") {
        return DiskKind::Optical;
    }
    if name.starts_with("vd")
        || name.starts_with("xvd")
        || canonical_has_component(root, dir, b"virtio")
    {
        return DiskKind::Virtual;
    }
    match read_trimmed(&dir.join("queue/rotational"), buf) {
        Ok(b"1") => DiskKind::Hdd,
        _ => DiskKind::Ssd,
    }
}

/// Udev `ID_MODEL` with `_` shown as spaces, else sysfs `device/model`. Both are sanitized.
fn read_model(
    dir: &Path,
    props: &UdevProps,
    buf: &mut Vec<u8>,
    text: &mut String,
) -> Option<Arc<str>> {
    if let Some(model) = props.get("ID_MODEL") {
        let spaced = model.replace('_', " ");
        if let Some(model) = clean(spaced.as_bytes(), text) {
            return Some(model);
        }
    }
    clean(read_trimmed(&dir.join("device/model"), buf).ok()?, text)
}

/// Sanitized and trimmed text, or `None` when nothing is left.
fn clean(raw: &[u8], text: &mut String) -> Option<Arc<str>> {
    sanitize_text(raw, NAME_MAX, text);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| Arc::from(trimmed))
}

/// Subdirectories of the disk directory that hold a `partition` file, sorted by number.
fn partitions(dir: &Path, buf: &mut Vec<u8>) -> Vec<BlockChild> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let part = dir.join(&name);
        let number = match read_u64(&part.join("partition"), buf) {
            Ok(n) => u32::try_from(n).unwrap_or(0),
            Err(CollectError::Parse { .. }) => 0,
            Err(_) => continue,
        };
        let (major, minor) = read_dev(&part, buf).unwrap_or((0, 0));
        out.push(BlockChild {
            name: Arc::from(name),
            major,
            minor,
            size_bytes: read_size(&part, buf),
            kind: ChildKind::Partition {
                number,
                start_sector: read_u64(&part.join("start"), buf).unwrap_or(0),
            },
        });
    }
    out.sort_by(|a, b| {
        let number = |c: &BlockChild| match c.kind {
            ChildKind::Partition { number, .. } => number,
            ChildKind::DeviceMapper { .. } => u32::MAX,
        };
        number(a).cmp(&number(b)).then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// The first entry of `slaves/` in sorted order, or `None` when it is empty or missing.
fn first_slave(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir.join("slaves"))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .min()
}

fn read_dev(dir: &Path, buf: &mut Vec<u8>) -> Result<(u32, u32), CollectError> {
    parse_dev(read_trimmed(&dir.join("dev"), buf)?)
}

/// `size` is in 512-byte sectors.
fn read_size(dir: &Path, buf: &mut Vec<u8>) -> u64 {
    read_u64(&dir.join("size"), buf)
        .unwrap_or(0)
        .saturating_mul(512)
}

/// True when the canonical path of `dir`, below the canonical sysfs root, has a component that
/// starts with `prefix`.
fn canonical_has_component(root: &SysRoot, dir: &Path, prefix: &[u8]) -> bool {
    let Ok(real) = std::fs::canonicalize(dir) else {
        return false;
    };
    let below = std::fs::canonicalize(&root.sys)
        .ok()
        .and_then(|sys| real.strip_prefix(sys).ok().map(Path::to_path_buf))
        .unwrap_or(real);
    below
        .components()
        .any(|c| c.as_os_str().as_bytes().starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn parse_dev_cases() {
        assert_eq!(parse_dev(b"259:0\n").unwrap(), (259, 0));
        assert_eq!(parse_dev(b"8:16").unwrap(), (8, 16));
        assert_eq!(parse_dev(b" 254:3 \n").unwrap(), (254, 3));
        for bad in [
            &b"259"[..],
            b"",
            b":",
            b"259:",
            b":0",
            b"a:0",
            b"259:0:1",
            b"-1:0",
            b"4294967296:0",
            b"259 :0",
            b"\xff:0",
        ] {
            assert!(
                matches!(parse_dev(bad), Err(CollectError::Parse { .. })),
                "{bad:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn prop_parse_dev_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..=32)) {
            let _ = parse_dev(&bytes);
        }

        #[test]
        fn prop_parse_dev_round_trip(major in any::<u32>(), minor in any::<u32>()) {
            prop_assert_eq!(
                parse_dev(format!("{major}:{minor}\n").as_bytes()).unwrap(),
                (major, minor)
            );
        }
    }
}
