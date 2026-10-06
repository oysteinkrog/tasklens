//! Static hardware and OS information for the Information tab.
//!
//! [`read_inventory`] is the one entry point for the `Inventory` worker (E12). Disk drives,
//! network adapters, graphics names, hostnames, desktop and session rows are not read here: they
//! come from the E04 samplers, the E12 D-Bus workers and the GPU probe.

pub mod cpuinfo;
pub mod displays;
pub mod dmi;
pub mod modules;
pub mod os_release;

pub use cpuinfo::{
    CpuFlags, CpuInfo, Lockdown, SecurityInfo, SecurityPaths, SecuritySource, Virtualization,
    VulnCounts, identifier, parse_cpuinfo, read_security,
};
pub use displays::{ConnectorInfo, ConnectorType, edid_monitor_name, read_connectors};
pub use dmi::{DmiInfo, Field, FirmwareType, read_dmi};
pub use modules::{MemoryModule, ModulesInfo, read_modules};
pub use os_release::{
    InstallDateSource, KernelInfo, OsRelease, package_manager_version, read_install_date,
    read_kernel, read_machine_id, read_os_release,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::sysroot::read_file;

/// Read limit for /proc/cpuinfo, in bytes. A 256-thread machine writes about 400 KiB.
const CPUINFO_MAX: usize = 1024 * 1024;

/// Where every inventory source lives: the host or a fixture directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryPaths {
    /// `/proc`
    pub proc: PathBuf,
    /// `/sys`
    pub sys: PathBuf,
    /// `/dev`
    pub dev: PathBuf,
    /// `/etc`
    pub etc: PathBuf,
    /// `/usr/lib`
    pub usr_lib: PathBuf,
    /// `/`, for the install date.
    pub root: PathBuf,
    /// `/run/udev/data/+dmi:id`
    pub udev_dmi: PathBuf,
    /// `/var/log/pacman.log`
    pub pacman_log: PathBuf,
    /// `$PATH` split on ':', absolute entries only.
    pub path_dirs: Vec<PathBuf>,
}

impl InventoryPaths {
    /// The host paths, with `path_dirs` from this process's `$PATH`.
    pub fn system() -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|p| {
                std::env::split_paths(&p)
                    .filter(|d| d.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path_dirs,
            ..Self::under(Path::new("/"))
        }
    }

    /// Every path joined below `root`. `path_dirs` is `<root>/usr/bin` and `<root>/bin`.
    pub fn under(root: &Path) -> Self {
        Self {
            proc: root.join("proc"),
            sys: root.join("sys"),
            dev: root.join("dev"),
            etc: root.join("etc"),
            usr_lib: root.join("usr/lib"),
            root: root.to_path_buf(),
            udev_dmi: root.join("run/udev/data/+dmi:id"),
            pacman_log: root.join("var/log/pacman.log"),
            path_dirs: vec![root.join("usr/bin"), root.join("bin")],
        }
    }
}

#[derive(Clone, Debug)]
pub struct InventoryRaw {
    pub cpu: CpuInfo,
    pub security: SecurityInfo,
    pub dmi: DmiInfo,
    pub os: Option<OsRelease>,
    pub kernel: KernelInfo,
    /// Never log this.
    pub machine_id: Option<Arc<str>>,
    pub install_date: Option<(i64, InstallDateSource)>,
    pub package_manager: Option<Arc<str>>,
    pub modules: ModulesInfo,
    /// Every DRM card's connectors, card order.
    pub connectors: Vec<ConnectorInfo>,
}

/// One full read for the Information tab. Runs on the blocking pool (E12). Never fails: every
/// reader degrades to `None`, `Missing` or `NotReported`. The package manager run can block for
/// up to 2 s.
pub fn read_inventory(p: &InventoryPaths) -> InventoryRaw {
    let mut cpu = CpuInfo::default();
    let mut buf = Vec::new();
    if read_file(&p.proc.join("cpuinfo"), &mut buf, CPUINFO_MAX).is_ok() {
        parse_cpuinfo(&buf, &mut cpu);
    }
    let security = read_security(
        &cpu,
        &SecurityPaths {
            sys: &p.sys,
            proc: &p.proc,
            dev: &p.dev,
        },
    );
    let connectors = drm_cards(&p.sys)
        .into_iter()
        .flat_map(|card| read_connectors(&p.sys, card))
        .collect();
    InventoryRaw {
        cpu,
        security,
        dmi: read_dmi(&p.sys),
        os: read_os_release(&p.etc, &p.usr_lib),
        kernel: read_kernel(),
        machine_id: read_machine_id(&p.etc),
        install_date: read_install_date(&p.root, &p.pacman_log),
        package_manager: package_manager_version(&p.path_dirs),
        modules: read_modules(&p.udev_dmi),
        connectors,
    }
}

/// Card numbers N from `<sys>/class/drm` entries `cardN` and `cardN-<connector>`, sorted and
/// without duplicates. Every vendor counts, since any card can drive a display.
fn drm_cards(sys: &Path) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(sys.join("class/drm")) else {
        return Vec::new();
    };
    let mut cards: Vec<u32> = entries
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let rest = name.strip_prefix("card")?;
            let digits = rest.split_once('-').map_or(rest, |(n, _)| n);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse().ok()
        })
        .collect();
    cards.sort_unstable();
    cards.dedup();
    cards
}
