//! Block device enumeration, kinds and children against the `disk-partitions-dm` hand fixture
//! and the captured `owner-desktop` fixture (ADR-016, PERF-047, R-140).
#![allow(clippy::unwrap_used)]
use std::fmt::Write as _;

use tl_collect::parse::block::{self, BlockDevice, ChildKind, DiskKind};

mod common;

fn disks(name: &str) -> Vec<BlockDevice> {
    let mut out = Vec::new();
    block::enumerate_disks(&common::fixture(name), &mut out).unwrap();
    out
}

fn disk<'a>(list: &'a [BlockDevice], name: &str) -> &'a BlockDevice {
    list.iter().find(|d| &*d.name == name).unwrap()
}

#[test]
fn kinds_and_exclusions() {
    let list = disks("disk-partitions-dm");
    let got: Vec<(&str, DiskKind)> = list.iter().map(|d| (&*d.name, d.kind)).collect();
    assert_eq!(
        got,
        [
            ("mmcblk0", DiskKind::Sd),
            ("nvme0n1", DiskKind::Nvme),
            ("sda", DiskKind::Hdd),
            ("sdb", DiskKind::Usb),
            ("sr0", DiskKind::Optical),
            ("vda", DiskKind::Virtual),
        ]
    );

    // R-140 item 3: udev ID_MODEL wins, with `_` shown as spaces; no udev file falls back to
    // sysfs.
    let sdb = disk(&list, "sdb");
    assert_eq!(sdb.model.as_deref(), Some("Flash Disk"));
    assert_eq!((sdb.major, sdb.minor), (8, 16));
    assert_eq!(sdb.size_bytes, 62_521_344 * 512);
    assert!(sdb.children.is_empty());
    assert_eq!(
        disk(&list, "sda").model.as_deref(),
        Some("WDC WD10EZEX-08W")
    );
    assert_eq!(disk(&list, "sr0").model, None);
    assert_eq!(disk(&list, "sr0").size_bytes, 0);
    assert!(list.iter().all(|d| !d.removable));
}

#[test]
fn children_partitions_and_dm() {
    let list = disks("disk-partitions-dm");
    let sda = disk(&list, "sda");
    assert_eq!(sda.size_bytes, 1_953_525_168 * 512);
    assert_eq!(sda.model.as_deref(), Some("WDC WD10EZEX-08W"));
    assert_eq!((sda.major, sda.minor), (8, 0));

    let names: Vec<&str> = sda.children.iter().map(|c| &*c.name).collect();
    assert_eq!(names, ["sda1", "sda2", "dm-0"]);
    assert_eq!(
        sda.children[0].kind,
        ChildKind::Partition {
            number: 1,
            start_sector: 2048
        }
    );
    assert_eq!((sda.children[0].major, sda.children[0].minor), (8, 1));
    assert_eq!(sda.children[0].size_bytes, 1_048_576 * 512);
    assert_eq!(
        sda.children[1].kind,
        ChildKind::Partition {
            number: 2,
            start_sector: 1_050_624
        }
    );
    assert_eq!(sda.children[1].size_bytes, 1_952_474_511 * 512);
    let dm = &sda.children[2];
    assert_eq!(
        dm.kind,
        ChildKind::DeviceMapper {
            dm_name: Some("cryptroot".into())
        }
    );
    assert_eq!((dm.major, dm.minor), (254, 0));
    assert_eq!(dm.size_bytes, 1_952_441_743 * 512);

    for other in list.iter().filter(|d| &*d.name != "sda") {
        assert!(other.children.is_empty(), "{}", other.name);
    }
}

#[test]
fn nvme_model_and_dev() {
    let list = disks("disk-partitions-dm");
    let nvme = disk(&list, "nvme0n1");
    assert_eq!(nvme.model.as_deref(), Some("Samsung SSD 980 PRO 1TB"));
    assert_eq!((nvme.major, nvme.minor), (259, 0));
    assert_eq!(nvme.size_bytes, 1_000_215_216 * 512);
}

#[test]
fn owner_desktop_snapshot() {
    let list = disks("owner-desktop");
    let names: Vec<&str> = list.iter().map(|d| &*d.name).collect();
    assert_eq!(names, ["nvme0n1", "nvme1n1", "sda"]);

    let mut report = String::new();
    for d in &list {
        writeln!(
            report,
            "{} dev={}:{} size={} kind={:?} model={:?} removable={}",
            d.name, d.major, d.minor, d.size_bytes, d.kind, d.model, d.removable
        )
        .unwrap();
        for c in &d.children {
            writeln!(
                report,
                "  {} dev={}:{} size={} {:?}",
                c.name, c.major, c.minor, c.size_bytes, c.kind
            )
            .unwrap();
        }
    }
    insta::assert_snapshot!(report);
}

#[test]
fn disk_temp_inputs() {
    let root = common::fixture("disk-partitions-dm");
    let nvme = block::disk_temp_input(&root, "nvme0n1").unwrap();
    assert!(nvme.ends_with("device/hwmon3/temp1_input"), "{nvme:?}");
    let sda = block::disk_temp_input(&root, "sda").unwrap();
    assert!(sda.ends_with("device/hwmon/hwmon5/temp1_input"), "{sda:?}");
    assert_eq!(block::disk_temp_input(&root, "vda"), None);
}
