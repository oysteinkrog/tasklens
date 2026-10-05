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
