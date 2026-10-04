//! The Partitions table of one disk (PERF-050, R-140 item 4): one row per partition ordered by
//! start, with unallocated gaps, holders, mounts, udev labels, filesystem types, usage, GPT type
//! names and attribute flags.
//!
//! Runs on enumeration only (every 5 s while the disk page is open), so allocation is fine here.
//! Udev values and sysfs attributes are untrusted. They are stored as read and the UI sanitizes
//! them before display. Labels and mount paths may hold user text: never log them.
use std::path::Path;
use std::sync::Arc;

use smallvec::SmallVec;
use tl_model::snapshot::{InvField, PartitionRow, PartitionTable};
use tl_model::validity::Validity;

use crate::parse::block::{BlockChild, BlockDevice, ChildKind, parse_dev};
use crate::parse::mountinfo::MountEntry;
use crate::parse::udev::{UdevProps, block_entry, read_udev_props};
use crate::sample::disk::FsStat;
use crate::sysroot::read_trimmed;
use crate::{CollectError, SysRoot};

/// The udev keys this module reads (R-140 item 1).
const UDEV_KEYS: [&str; 5] = [
    "ID_FS_TYPE",
    "ID_FS_LABEL",
    "ID_PART_TABLE_TYPE",
    "ID_PART_ENTRY_TYPE",
    "ID_PART_ENTRY_FLAGS",
];

const SECTOR: u64 = 512;
const MIB: u64 = 1024 * 1024;
/// Gaps before the first partition are counted from sector 2048.
const FIRST_USABLE: u64 = 2048 * SECTOR;
const READ_ONLY_BIT: u64 = 1 << 60;

/// GPT attribute bits and their names, in display order.
const GPT_FLAGS: [(u64, &str); 5] = [
    (1, "Required"),
    (1 << 2, "Boot"),
    (READ_ONLY_BIT, "Read-only"),
    (1 << 62, "Hidden"),
    (1 << 63, "No automount"),
];

/// GPT partition type GUIDs (lower case) and their names.
const GPT_TYPES: [(&str, &str); 12] = [
    ("c12a7328-f81f-11d2-ba4b-00a0c93ec93b", "EFI system"),
    ("0fc63daf-8483-4772-8e79-3d69d8477de4", "Linux filesystem"),
    (
        "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
        "Linux root (x86-64)",
    ),
    ("933ac7e1-2eb4-4f13-b844-0e14e2aef915", "Linux home"),
    ("0657fd6d-a4ab-43c4-84e5-0933c84b4f4f", "Linux swap"),
    ("e6d6d379-f507-44c2-a23c-238f2a3df928", "Linux LVM"),
    ("a19d880f-05fc-4d3b-a006-743f0f84911e", "Linux RAID"),
    ("ca7d7ccb-63ed-4c53-861c-1742536059cc", "LUKS"),
    (
        "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7",
        "Microsoft basic data",
    ),
    ("e3c9e316-0b5c-4db8-817d-f92df00215ae", "Microsoft reserved"),
    ("de94bba4-06d1-4d40-a16a-bfd50179d6ac", "Windows recovery"),
    ("21686148-6449-6e6f-744e-656564454649", "BIOS boot"),
];

/// Builds the table of one disk. The udev database is read from `root.run_path("udev/data")`;
/// `udev` is a reusable buffer.
///
/// Missing or unreadable files leave fields at their fallback. Never panics.
pub fn partition_table(
    root: &SysRoot,
    disk: &BlockDevice,
    mounts: &[MountEntry],
    statvfs: &mut dyn FnMut(&str) -> Result<FsStat, CollectError>,
    udev: &mut UdevProps,
) -> PartitionTable {
    read_entry(root, disk.major, disk.minor, udev);
    let kind: Arc<str> = match udev.get("ID_PART_TABLE_TYPE").map(str::trim) {
        None | Some("") => Arc::from("none"),
        Some(t) if t.eq_ignore_ascii_case("gpt") => Arc::from("GPT"),
        Some(t) if t.eq_ignore_ascii_case("dos") => Arc::from("MBR"),
        Some(t) => Arc::from(t.to_ascii_uppercase()),
    };

    let mut parts: Vec<(&BlockChild, u64)> = disk
        .children
        .iter()
        .filter_map(|c| match c.kind {
            ChildKind::Partition { start_sector, .. } => Some((c, start_sector)),
            ChildKind::DeviceMapper { .. } => None,
        })
        .collect();
    parts.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.name.cmp(&b.0.name)));

    let mut rows = Vec::with_capacity(parts.len() * 2 + 1);
    let mut buf = Vec::new();
    let mut cursor = FIRST_USABLE;
    for &(part, start_sector) in &parts {
        let start = start_sector.saturating_mul(SECTOR);
        push_gap(&mut rows, cursor, start);
        cursor = cursor.max(start.saturating_add(part.size_bytes));
        rows.push(partition_row(
            root, disk, part, start, mounts, statvfs, udev, &mut buf,
        ));
    }
    if !parts.is_empty() {
        push_gap(&mut rows, cursor, disk.size_bytes);
    }
    PartitionTable {
        kind,
        rows: rows.into(),
    }
}

/// GPT type GUID to name (case-insensitive), or "GPT partition {guid}"; the MBR form "0x83" to
/// "MBR 0x83"; an empty value to "Unknown".
pub fn type_name(part_entry_type: &str) -> Arc<str> {
    let value = part_entry_type.trim();
    if value.is_empty() {
        return Arc::from("Unknown");
    }
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        && !hex.is_empty()
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
        && let Ok(byte) = u8::from_str_radix(hex, 16)
    {
        return Arc::from(format!("MBR 0x{byte:02X}"));
    }
    match GPT_TYPES
        .iter()
        .find(|(guid, _)| guid.eq_ignore_ascii_case(value))
    {
        Some((_, name)) => Arc::from(*name),
        None => Arc::from(format!("GPT partition {value}")),
    }
}

/// GPT attribute bits (hex, with or without "0x") to names. Text that is not hex gives no flags.
pub fn gpt_flags(hex: &str) -> SmallVec<[&'static str; 4]> {
    flag_names(parse_flag_bits(hex))
}

fn parse_flag_bits(hex: &str) -> u64 {
    let hex = hex.trim();
    let digits = hex
        .strip_prefix("0x")
        .or_else(|| hex.strip_prefix("0X"))
        .unwrap_or(hex);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return 0;
    }
    u64::from_str_radix(digits, 16).unwrap_or(0)
}

fn flag_names(bits: u64) -> SmallVec<[&'static str; 4]> {
    GPT_FLAGS
        .iter()
        .filter(|(bit, _)| bits & bit != 0)
        .map(|(_, name)| *name)
        .collect()
}

/// An "Unallocated" row for the space from `from` to `to` when it is more than 1 MiB.
fn push_gap(rows: &mut Vec<PartitionRow>, from: u64, to: u64) {
    let size = to.saturating_sub(from);
    if size <= MIB {
        return;
    }
    rows.push(PartitionRow {
        name: Arc::from(""),
        start_bytes: from,
        size_bytes: size,
        holder: None,
        mount: None,
        extra_mounts: Arc::from([]),
        label: None,
        fs_type: None,
        usage_pct: invalid(Validity::Unsupported),
        flags: Arc::from([]),
        type_name: Arc::from("Unallocated"),
        read_only: false,
        unallocated: true,
    });
}

#[allow(clippy::too_many_arguments)]
fn partition_row(
    root: &SysRoot,
    disk: &BlockDevice,
    part: &BlockChild,
    start: u64,
    mounts: &[MountEntry],
    statvfs: &mut dyn FnMut(&str) -> Result<FsStat, CollectError>,
    udev: &mut UdevProps,
    buf: &mut Vec<u8>,
) -> PartitionRow {
    let dir = root.sys_path(&format!("block/{}/{}", disk.name, part.name));

    let holder_name = first_entry(&dir.join("holders"));
    let mut holder = None;
    let mut holder_dev = None;
    if let Some(name) = &holder_name {
        let holder_dir = root.sys_path(&format!("block/{name}"));
        holder = Some(match read_trimmed(&holder_dir.join("dm/name"), buf) {
            Ok(dm) if !dm.is_empty() => {
                Arc::from(format!("{} → {name}", String::from_utf8_lossy(dm)))
            }
            _ => Arc::from(name.as_str()),
        });
        holder_dev = read_trimmed(&holder_dir.join("dev"), buf)
            .and_then(parse_dev)
            .ok();
    }

    read_entry(root, part.major, part.minor, udev);
    let mut label = value(udev, "ID_FS_LABEL");
    let mut fs_type = value(udev, "ID_FS_TYPE");
    let type_name = type_name(udev.get("ID_PART_ENTRY_TYPE").unwrap_or(""));
    let mut bits = parse_flag_bits(udev.get("ID_PART_ENTRY_FLAGS").unwrap_or(""));

    let luks = fs_type
        .as_deref()
        .is_some_and(|t| t.eq_ignore_ascii_case("crypto_LUKS"));
    if luks && let Some((major, minor)) = holder_dev {
        read_entry(root, major, minor, udev);
        if let Some(inner) = value(udev, "ID_FS_TYPE") {
            fs_type = Some(inner);
            label = value(udev, "ID_FS_LABEL");
        }
    }

    if matches!(read_trimmed(&dir.join("ro"), buf), Ok(b"1")) {
        bits |= READ_ONLY_BIT;
    }

    let own = (part.major, part.minor);
    let mut matching = mounts.iter().filter(|m| {
        let dev = (m.major, m.minor);
        dev.0 != 0 && (dev == own || Some(dev) == holder_dev)
    });
    let mount = matching.next().map(|m| m.mount_point.clone());
    let extra_mounts: Arc<[Arc<str>]> = matching.map(|m| m.mount_point.clone()).collect();

    let usage_pct = match &mount {
        None => invalid(Validity::Unsupported),
        Some(path) => match statvfs(path) {
            Ok(st) if st.blocks > 0 => {
                let used = st.blocks.saturating_sub(st.bfree);
                InvField {
                    value: (100.0 * used as f64 / st.blocks as f64) as f32,
                    validity: Validity::Valid,
                }
            }
            Ok(_) => invalid(Validity::Unsupported),
            Err(err) => invalid(err.validity()),
        },
    };

    PartitionRow {
        name: part.name.clone(),
        start_bytes: start,
        size_bytes: part.size_bytes,
        holder,
        mount,
        extra_mounts,
        label,
        fs_type,
        usage_pct,
        flags: flag_names(bits).into_iter().map(Arc::from).collect(),
        type_name,
        read_only: bits & READ_ONLY_BIT != 0,
        unallocated: false,
    }
}

/// Reads the udev entry of a block device into `udev`. A file that cannot be read leaves it empty.
fn read_entry(root: &SysRoot, major: u32, minor: u32, udev: &mut UdevProps) {
    let path = root.run_path(&format!("udev/data/{}", block_entry(major, minor)));
    if read_udev_props(&path, &UDEV_KEYS, udev).is_err() {
        udev.clear();
    }
}

/// The value of `key`, or `None` when it is missing or empty.
fn value(udev: &UdevProps, key: &str) -> Option<Arc<str>> {
    udev.0
        .iter()
        .find(|(k, v)| &**k == key && !v.is_empty())
        .map(|(_, v)| v.clone())
}

/// The first entry of `dir` in sorted order, or `None` when it is empty, missing or not UTF-8.
fn first_entry(dir: &Path) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .min()
}

fn invalid(validity: Validity) -> InvField<f32> {
    InvField {
        value: 0.0,
        validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::block::{DiskKind, enumerate_disks};
    use crate::parse::mountinfo::parse_mountinfo;
    use proptest::prelude::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/disk-partitions-dm")
    }

    fn sda(root: &SysRoot) -> BlockDevice {
        let mut disks = Vec::new();
        enumerate_disks(root, &mut disks).unwrap();
        disks.into_iter().find(|d| &*d.name == "sda").unwrap()
    }

    fn fixture_mounts() -> Vec<MountEntry> {
        let mut out = Vec::new();
        let bytes = fs::read(fixture_dir().join("proc/self/mountinfo")).unwrap();
        parse_mountinfo(&bytes, &mut out).unwrap();
        out
    }

    const STAT: FsStat = FsStat {
        blocks: 1000,
        bfree: 250,
        bavail: 200,
        frsize: 4096,
    };

    fn build(root: &SysRoot, disk: &BlockDevice, mounts: &[MountEntry]) -> PartitionTable {
        let mut statvfs = |path: &str| match path {
            "/" => Err(CollectError::PermissionDenied),
            _ => Ok(STAT),
        };
        partition_table(root, disk, mounts, &mut statvfs, &mut UdevProps::default())
    }

    fn strs(list: &[Arc<str>]) -> Vec<&str> {
        list.iter().map(|s| &**s).collect()
    }

    #[test]
    fn gpt_rows_ordered_with_holder_and_mounts() {
        let root = SysRoot::at(&fixture_dir());
        let disk = sda(&root);
        let mut calls = Vec::new();
        let mut statvfs = |path: &str| {
            calls.push(path.to_owned());
            if path == "/" {
                Err(CollectError::PermissionDenied)
            } else {
                Ok(STAT)
            }
        };
        let table = partition_table(
            &root,
            &disk,
            &fixture_mounts(),
            &mut statvfs,
            &mut UdevProps::default(),
        );
        assert_eq!(&*table.kind, "GPT");
        assert_eq!(table.rows.len(), 2, "{:?}", table.rows);

        let efi = &table.rows[0];
        assert_eq!(&*efi.name, "sda1");
        assert_eq!(efi.start_bytes, 2048 * 512);
        assert_eq!(efi.size_bytes, 1_048_576 * 512);
        assert_eq!(&*efi.type_name, "EFI system");
        assert_eq!(strs(&efi.flags), ["Required"]);
        assert_eq!(efi.label.as_deref(), Some("EFI"));
        assert_eq!(efi.fs_type.as_deref(), Some("vfat"));
        assert_eq!(efi.mount.as_deref(), Some("/boot"));
        assert_eq!(strs(&efi.extra_mounts), ["/mnt/boot copy"]);
        assert_eq!(efi.holder, None);
        assert_eq!(efi.usage_pct.validity, Validity::Valid);
        assert!((efi.usage_pct.value - 75.0).abs() < 1e-4);
        assert!(!efi.read_only && !efi.unallocated);

        let root_part = &table.rows[1];
        assert_eq!(&*root_part.name, "sda2");
        assert_eq!(root_part.start_bytes, 1_050_624 * 512);
        assert_eq!(root_part.holder.as_deref(), Some("cryptroot → dm-0"));
        assert_eq!(root_part.mount.as_deref(), Some("/"));
        assert!(root_part.extra_mounts.is_empty());
        assert_eq!(&*root_part.type_name, "LUKS");
        // The partition is LUKS, so the holder's udev entry gives the filesystem.
        assert_eq!(root_part.fs_type.as_deref(), Some("ext4"));
        assert_eq!(root_part.label.as_deref(), Some("root"));
        assert_eq!(root_part.usage_pct.validity, Validity::Restricted);
        assert!(root_part.flags.is_empty());

        assert!(
            table
                .rows
                .windows(2)
                .all(|w| w[0].start_bytes <= w[1].start_bytes)
        );
        // One statvfs per mounted partition, on its first mount.
        assert_eq!(calls, ["/boot", "/"]);
    }

    #[test]
    fn unallocated_gap_over_1mib() {
        let root = SysRoot::at(&fixture_dir());
        let mut disk = sda(&root);
        let gap_sectors = 4 * MIB / SECTOR;
        for child in &mut disk.children {
            if let ChildKind::Partition {
                number: 2,
                start_sector,
            } = &mut child.kind
            {
                *start_sector += gap_sectors;
                child.size_bytes -= gap_sectors * SECTOR;
            }
        }
        let table = build(&root, &disk, &[]);
        let names: Vec<&str> = table.rows.iter().map(|r| &*r.name).collect();
        assert_eq!(names, ["sda1", "", "sda2"]);
        let gap = &table.rows[1];
        assert!(gap.unallocated);
        assert_eq!(&*gap.type_name, "Unallocated");
        assert_eq!(gap.start_bytes, 1_050_624 * 512);
        assert_eq!(gap.size_bytes, 4 * MIB);
        assert_eq!(gap.usage_pct.validity, Validity::Unsupported);
        assert_eq!((gap.mount.clone(), gap.label.clone()), (None, None));
        assert!(gap.flags.is_empty() && gap.extra_mounts.is_empty());

        // A gap of exactly 1 MiB is not shown; one past the last partition is.
        let mut disk = sda(&root);
        disk.size_bytes = 1_953_525_135 * 512 + MIB;
        assert_eq!(build(&root, &disk, &[]).rows.len(), 2);
        disk.size_bytes += 1;
        let table = build(&root, &disk, &[]);
        let last = table.rows.last().unwrap();
        assert!(last.unallocated);
        assert_eq!(last.start_bytes, 1_953_525_135 * 512);
        assert_eq!(last.size_bytes, MIB + 1);

        // Before the first partition, counted from sector 2048.
        let mut disk = sda(&root);
        disk.children.retain(|c| &*c.name != "sda1");
        let table = build(&root, &disk, &[]);
        assert!(table.rows[0].unallocated);
        assert_eq!(table.rows[0].start_bytes, FIRST_USABLE);
        assert_eq!(table.rows[0].size_bytes, 1_048_576 * 512);
    }

    #[test]
    fn type_names_gpt_and_mbr() {
        assert_eq!(&*type_name("0x83"), "MBR 0x83");
        assert_eq!(&*type_name("0X7"), "MBR 0x07");
        assert_eq!(&*type_name("0xef"), "MBR 0xEF");
        assert_eq!(
            &*type_name("C12A7328-F81F-11D2-BA4B-00A0C93EC93B"),
            "EFI system"
        );
        assert_eq!(
            &*type_name("21686148-6449-6e6f-744e-656564454649"),
            "BIOS boot"
        );
        let guid = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(&*type_name(guid), format!("GPT partition {guid}"));
        assert_eq!(&*type_name(""), "Unknown");
        assert_eq!(&*type_name("0x"), "GPT partition 0x");
        assert_eq!(&*type_name("0x1ff"), "GPT partition 0x1ff");
        for (guid, name) in GPT_TYPES {
            assert_eq!(&*type_name(guid), name);
        }
    }

    /// A small tree with one disk `sdx` (8:32) and one partition `sdx1` (8:33).
    fn one_partition(udev: &str, ro: &str) -> (tempfile::TempDir, BlockDevice) {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("sys/block/sdx/sdx1");
        fs::create_dir_all(&part).unwrap();
        fs::write(part.join("ro"), ro).unwrap();
        let data = dir.path().join("run/udev/data");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("b8:33"), udev).unwrap();
        let disk = BlockDevice {
            name: Arc::from("sdx"),
            major: 8,
            minor: 32,
            size_bytes: 4096 * SECTOR,
            kind: DiskKind::Ssd,
            model: None,
            removable: false,
            children: vec![BlockChild {
                name: Arc::from("sdx1"),
                major: 8,
                minor: 33,
                size_bytes: 2048 * SECTOR,
                kind: ChildKind::Partition {
                    number: 1,
                    start_sector: 2048,
                },
            }],
        };
        (dir, disk)
    }

    #[test]
    fn gpt_flag_bits_and_ro() {
        let (dir, disk) = one_partition("E:ID_PART_ENTRY_FLAGS=0x1000000000000004\n", "1\n");
        let table = build(&SysRoot::at(dir.path()), &disk, &[]);
        let row = &table.rows[0];
        assert_eq!(strs(&row.flags), ["Boot", "Read-only"]);
        assert!(row.read_only);

        // `ro` alone sets the flag; bit 60 alone sets `read_only`.
        let (dir, disk) = one_partition("E:ID_PART_ENTRY_FLAGS=0x4\n", "1\n");
        let row = &build(&SysRoot::at(dir.path()), &disk, &[]).rows[0];
        assert_eq!(strs(&row.flags), ["Boot", "Read-only"]);
        let (dir, disk) = one_partition("E:ID_PART_ENTRY_FLAGS=0x1000000000000000\n", "0\n");
        let row = &build(&SysRoot::at(dir.path()), &disk, &[]).rows[0];
        assert!(row.read_only);
        let (dir, disk) = one_partition("E:ID_PART_ENTRY_FLAGS=0x0\n", "0\n");
        let row = &build(&SysRoot::at(dir.path()), &disk, &[]).rows[0];
        assert!(!row.read_only && row.flags.is_empty());

        assert_eq!(
            gpt_flags("0xD000000000000005").as_slice(),
            ["Required", "Boot", "Read-only", "Hidden", "No automount"]
        );
        assert_eq!(gpt_flags("8000000000000000").as_slice(), ["No automount"]);
        assert!(gpt_flags("zz").is_empty());
        assert!(gpt_flags("").is_empty());
        assert!(gpt_flags("0x1ffffffffffffffff").is_empty(), "overflow");
    }

    #[test]
    fn no_udev_entry_defaults() {
        let empty = tempfile::tempdir().unwrap();
        let root = SysRoot {
            run: empty.path().to_path_buf(),
            ..SysRoot::at(&fixture_dir())
        };
        let disk = sda(&root);
        let table = build(&root, &disk, &fixture_mounts());
        assert_eq!(&*table.kind, "none");
        assert_eq!(table.rows.len(), 2);
        for row in table.rows.iter() {
            assert_eq!(&*row.type_name, "Unknown");
            assert_eq!(row.label, None);
            assert_eq!(row.fs_type, None);
            assert!(row.flags.is_empty());
        }
        // Mounts and holders do not depend on udev.
        assert_eq!(table.rows[0].mount.as_deref(), Some("/boot"));
        assert_eq!(table.rows[1].holder.as_deref(), Some("cryptroot → dm-0"));

        // No partitions: no rows, not even a gap.
        let mut bare = disk.clone();
        bare.children.clear();
        assert!(build(&root, &bare, &[]).rows.is_empty());
    }

    fn udev_text() -> impl Strategy<Value = Vec<u8>> {
        let line = (
            prop::sample::select(UDEV_KEYS.to_vec()),
            prop_oneof!["0[xX][0-9a-fA-F]{0,20}", "[0-9a-fA-F-]{0,40}", "\\PC{0,24}",],
        )
            .prop_map(|(k, v)| format!("E:{k}={v}\n").into_bytes());
        prop_oneof![
            prop::collection::vec(line, 0..6).prop_map(|lines| lines.concat()),
            prop::collection::vec(any::<u8>(), 0..128),
        ]
    }

    fn sys_text() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            "[0-9]{0,22}\n?".prop_map(String::into_bytes),
            "[0-9]{1,3}:[0-9]{1,3}".prop_map(String::into_bytes),
            prop::collection::vec(any::<u8>(), 0..32),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn no_panic_on_mutated_inputs(
            disk_udev in udev_text(),
            part_udev in udev_text(),
            holder_udev in udev_text(),
            sys in prop::collection::vec(sys_text(), 8),
            holder in "[a-z0-9-]{1,8}",
            stat in (any::<u64>(), any::<u64>()),
            mounts in prop::collection::vec((0u32..300, 0u32..4, "/[a-z]{0,4}"), 0..5),
        ) {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path();
            let disk_dir = base.join("sys/block/sdx");
            for part in ["sdx1", "sdx2"] {
                fs::create_dir_all(disk_dir.join(part).join("holders")).unwrap();
                fs::write(disk_dir.join(part).join("partition"), &sys[0]).unwrap();
            }
            fs::write(disk_dir.join("dev"), &sys[1]).unwrap();
            fs::write(disk_dir.join("size"), &sys[2]).unwrap();
            fs::write(disk_dir.join("sdx1/start"), &sys[3]).unwrap();
            fs::write(disk_dir.join("sdx1/size"), &sys[4]).unwrap();
            fs::write(disk_dir.join("sdx1/dev"), b"8:33").unwrap();
            fs::write(disk_dir.join("sdx1/ro"), &sys[5]).unwrap();
            fs::write(disk_dir.join("sdx1/holders").join(&holder), b"").unwrap();
            fs::write(disk_dir.join("sdx2/start"), &sys[6]).unwrap();
            fs::write(disk_dir.join("sdx2/dev"), &sys[7]).unwrap();
            let holder_dir = base.join("sys/block").join(&holder);
            fs::create_dir_all(holder_dir.join("dm")).unwrap();
            fs::write(holder_dir.join("dm/name"), &part_udev).unwrap();
            fs::write(holder_dir.join("dev"), b"254:0").unwrap();
            let data = base.join("run/udev/data");
            fs::create_dir_all(&data).unwrap();
            if let Ok((major, minor)) = parse_dev(&sys[1]) {
                fs::write(data.join(block_entry(major, minor)), &disk_udev).unwrap();
            }
            fs::write(data.join("b8:33"), &part_udev).unwrap();
            fs::write(data.join("b254:0"), &holder_udev).unwrap();

            let root = SysRoot::at(base);
            let mut disks = Vec::new();
            enumerate_disks(&root, &mut disks).unwrap();
            let mounts: Vec<MountEntry> = mounts
                .into_iter()
                .map(|(major, minor, path)| MountEntry {
                    major,
                    minor,
                    mount_point: Arc::from(path),
                    fs_type: Arc::from("ext4"),
                })
                .collect();
            let mut statvfs = |_: &str| -> Result<FsStat, CollectError> {
                Ok(FsStat { blocks: stat.0, bfree: stat.1, bavail: 0, frsize: 4096 })
            };
            let mut udev = UdevProps::default();
            for disk in &disks {
                let table = partition_table(&root, disk, &mounts, &mut statvfs, &mut udev);
                prop_assert!(table.rows.windows(2).all(|w| w[0].start_bytes <= w[1].start_bytes));
                for row in table.rows.iter() {
                    prop_assert!(!row.usage_pct.validity.is_valid() || (0.0..=100.0).contains(&row.usage_pct.value));
                }
            }
        }

        #[test]
        fn names_never_panic(text in "\\PC{0,48}") {
            let _ = type_name(&text);
            let _ = gpt_flags(&text);
        }
    }
}
