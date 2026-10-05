//! Inventory readers against fixtures and the real machine (E06-015; E06-A3 adds fixture tests).

use std::path::Path;

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
