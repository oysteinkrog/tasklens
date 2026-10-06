//! Inventory readers against fixtures and the real machine (E06-015; E06-A3 adds fixture tests).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tl_collect::inventory::cpuinfo::{CpuInfo, SecurityPaths, parse_cpuinfo, read_security};

/// Settles U-27: which security sources a normal user can read. Run as uid 1000 with
/// `cargo test -p tl-collect --test fixture_inventory -- --ignored --nocapture`.
#[test]
#[ignore = "reads the real /sys, /proc and /dev"]
fn real_security_readability() {
    let mut cpu = CpuInfo::default();
    parse_cpuinfo(&std::fs::read("/proc/cpuinfo").unwrap(), &mut cpu);
    let info = read_security(
        &cpu,
        &SecurityPaths {
            sys: Path::new("/sys"),
            proc: Path::new("/proc"),
            dev: Path::new("/dev"),
        },
    );
    eprintln!("flags: {:?}", cpu.flags);
    eprintln!("virtualization: {:?}", info.virtualization);
    eprintln!("hypervisor: {:?}", info.hypervisor);
    eprintln!("secure_boot: {:?}", info.secure_boot);
    eprintln!("lockdown: {:?}", info.lockdown);
    eprintln!("memory_encryption: {:?}", info.memory_encryption);
    eprintln!("user_shadow_stack: {:?}", info.user_shadow_stack);
    eprintln!("iommu: {:?}", info.iommu);
    eprintln!("vulns: {:?}", info.vulns);
    eprintln!("unreadable: {:?}", info.unreadable);
}

/// PERF-102: `board_serial` is mode 0400, so uid 1000 sees `Restricted`. The value is never read
/// or printed. Run as uid 1000 with
/// `cargo test -p tl-collect --test fixture_inventory -- --ignored real_board_serial_restricted`.
#[test]
#[ignore = "reads the real /sys"]
fn real_board_serial_restricted() {
    use tl_collect::inventory::dmi::{Field, read_dmi};
    let info = read_dmi(Path::new("/sys"));
    assert!(
        info.board_serial == Field::Restricted,
        "board_serial is not Restricted"
    );
}

/// Settles U-26: whether sysfs `enabled` is reported for every monitor (NVIDIA 615). Prints every
/// connector of every card. Run with
/// `cargo test -p tl-collect --test fixture_inventory -- --ignored real_connectors_listed --nocapture`.
#[test]
#[ignore = "reads the real /sys"]
fn real_connectors_listed() {
    use tl_collect::inventory::displays::read_connectors;
    let mut cards: Vec<u32> = std::fs::read_dir("/sys/class/drm")
        .unwrap()
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            name.strip_prefix("card")?.parse().ok()
        })
        .collect();
    cards.sort_unstable();
    for card in cards {
        for c in read_connectors(Path::new("/sys"), card) {
            eprintln!("{c:?}");
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The fixture read with the host-dependent fields fixed: `kernel` comes from `uname`, and the
/// birth time of the fixture directory is the checkout time.
fn dmi_desktop() -> tl_collect::inventory::InventoryRaw {
    use tl_collect::inventory::{InstallDateSource, InventoryPaths, KernelInfo, read_inventory};
    let mut raw = read_inventory(&InventoryPaths::under(&fixture("dmi-desktop")));
    raw.kernel = KernelInfo {
        release: Arc::from("6.17.1-2-cachyos"),
        version: Arc::from("#1 SMP PREEMPT_DYNAMIC"),
        machine: Arc::from("x86_64"),
        nis_domain: None,
    };
    if let Some((_, InstallDateSource::RootBirthTime)) = raw.install_date {
        raw.install_date = Some((0, InstallDateSource::RootBirthTime));
    }
    raw
}

#[test]
fn fixture_dmi_desktop_snapshot() {
    insta::assert_debug_snapshot!("dmi_desktop", dmi_desktop());
}

/// D-007 regression test: no serial reaches the result.
#[test]
fn no_serial_anywhere() {
    let root = fixture("dmi-desktop");
    let product_serial = std::fs::read_to_string(root.join("sys/class/dmi/id/product_serial"))
        .expect("fixture product_serial");
    assert!(product_serial.contains("SERIAL-MUST-NOT-APPEAR"));
    let udev = std::fs::read_to_string(root.join("run/udev/data/+dmi:id")).expect("fixture udev");
    assert!(udev.contains("MEMORY_DEVICE_0_SERIAL_NUMBER=SERIAL-MUST-NOT-APPEAR"));

    let text = format!("{:?}", dmi_desktop());
    assert!(!text.contains("SERIAL-MUST-NOT-APPEAR"));
}

#[test]
fn empty_root_no_panic() {
    use tl_collect::inventory::{Field, InventoryPaths, ModulesInfo, read_inventory};
    let dir = tempfile::tempdir().expect("tempdir");
    let raw = read_inventory(&InventoryPaths::under(dir.path()));
    assert_eq!(raw.os, None);
    assert_eq!(raw.modules, ModulesInfo::NotReported);
    let d = &raw.dmi;
    for field in [
        &d.bios_vendor,
        &d.bios_version,
        &d.bios_date,
        &d.bios_release,
        &d.board_vendor,
        &d.board_name,
        &d.board_version,
        &d.board_serial,
        &d.sys_vendor,
        &d.product_name,
    ] {
        assert_eq!(field, &Field::Missing);
    }
    assert!(raw.connectors.is_empty());
    assert_eq!(raw.machine_id, None);
    assert_eq!(raw.package_manager, None);
}

#[test]
fn amd_through_reexports() {
    use tl_collect::gpu_drm::{AmdSample, VENDOR_AMD, discover_cards, read_amd};
    let sys = fixture("drm-amd").join("sys");
    let cards = discover_cards(&sys);
    let amd: Vec<_> = cards.iter().filter(|c| c.vendor == VENDOR_AMD).collect();
    assert_eq!(amd.len(), 1, "{cards:?}");
    let mut sample = AmdSample::default();
    read_amd(amd[0], &sys, &mut sample);
    assert_eq!(sample.gpu_busy_pct, Some(37));
}
