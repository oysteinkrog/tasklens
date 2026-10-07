//! Row builders for the ten Information sections (UI-SYS-086 to UI-SYS-095; PERF-095 to
//! PERF-104, PERF-069, APP-041, D-007, R-050, table d).
//!
//! Each builder turns the snapshot into [`InfoRow`]s with fixed labels, formats and hide rules,
//! so the section entity only draws. The page calls them only when INVENTORY, SYSTEM or DEVICES
//! changes; they append to the caller's `Vec` and format each value once. Memory modules, Disk
//! drives, Graphics and Network adapters give one sub-block per item, numbered from 1.
//!
//! A value whose field is not Valid or Stale carries that validity and no text; the grid shows
//! the word (UI-SYS-082). Unsupported inventory fields read "Not reported" (R-050), and so does
//! a Valid field with empty text. Values are local machine data and are never logged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, SharedString};
use smallvec::SmallVec;
use tl_model::devices::DeviceCatalog;
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{
    DisplayMode, FirmwareType, InvField, InventorySnapshot, NetworkAdapter, SystemSnapshot,
};
use tl_model::system::Metric;
use tl_model::units;
use tl_model::validity::Validity;

use super::section::InfoRow;
use crate::components::kv_grid::{TlValue, UnsupportedWord};
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;

/// What the builders read.
pub struct InfoInputs<'a> {
    pub inv: &'a InventorySnapshot,
    pub sys: &'a SystemSnapshot,
    pub devices: &'a DeviceCatalog,
    /// The labels and words, translated once per strings version.
    pub words: &'a InfoWords,
}

/// One label or word of [`InfoWords`]. The order matches [`KEYS`].
#[derive(Clone, Copy)]
enum W {
    OsName,
    OsVersion,
    OsBuild,
    OsKernel,
    OsKernelVersion,
    OsArchitecture,
    OsInstallDate,
    OsPackageManager,
    OsMachineId,
    OsRolling,
    CpuName,
    CpuVendor,
    CpuArchitecture,
    CpuIdentifier,
    CpuCores,
    CpuLogical,
    CpuBaseSpeed,
    CpuMaxSpeed,
    CpuMicrocode,
    CpuFlagsSummary,
    CpuUnknown,
    CpuYes,
    CpuNo,
    CpuYesAmdV,
    CpuYesVtX,
    SysComputerName,
    SysUserName,
    SysDomain,
    SysUptime,
    SysBootTime,
    SysProcessCount,
    SysThreadCount,
    SysHandleCount,
    SysHandleCountTip,
    SysDesktop,
    SysWindowing,
    SysSessionType,
    SysVirtualTerminal,
    BiosFirmwareType,
    BiosUefi,
    BiosLegacy,
    BiosVendor,
    BiosVersion,
    BiosDate,
    BiosRelease,
    BiosSecureBoot,
    BiosEnabled,
    BiosDisabled,
    BoardManufacturer,
    BoardProduct,
    BoardVersion,
    BoardSystemVendor,
    BoardSystemProduct,
    BoardChassisType,
    BoardSerial,
    BoardSerialRestrictedTip,
    MemTotal,
    MemAvailable,
    MemUsage,
    MemCommitted,
    MemCommitLimit,
    MemPeak,
    MemPageSize,
    MemHugePageSize,
    MemSwapTotal,
    MemZram,
    MemZramNone,
    ModModule,
    ModSlot,
    ModCapacity,
    ModSpeed,
    ModType,
    ModFormFactor,
    ModManufacturer,
    ModPartNumber,
    ModNone,
    DiskModel,
    DiskType,
    DiskInterface,
    DiskCapacity,
    DiskSpace,
    DiskVolumes,
    DiskSerial,
    DiskSsd,
    DiskHdd,
    DiskVolumesNone,
    GpuName,
    GpuDriverVersion,
    GpuKernelModule,
    GpuVulkan,
    GpuOpenGl,
    GpuTemperature,
    GpuTemperatureSource,
    GpuDedicated,
    GpuShared,
    GpuResolution,
    GpuUnknown,
    NetConnection,
    NetDetails,
    NetIp,
    NetMac,
    NetUnknown,
    NetIpNone,
}

const KEYS: [&str; 103] = [
    "information-os-name",
    "information-os-version",
    "information-os-build",
    "information-os-kernel",
    "information-os-kernel-version",
    "information-os-architecture",
    "information-os-install-date",
    "information-os-package-manager",
    "information-os-machine-id",
    "information-os-rolling",
    "information-cpu-name",
    "information-cpu-vendor",
    "information-cpu-architecture",
    "information-cpu-identifier",
    "information-cpu-cores",
    "information-cpu-logical-processors",
    "information-cpu-base-speed",
    "information-cpu-max-speed",
    "information-cpu-microcode",
    "information-cpu-flags-summary",
    "information-cpu-unknown-processor",
    "information-cpu-yes",
    "information-cpu-no",
    "information-cpu-yes-amd-v",
    "information-cpu-yes-vt-x",
    "information-system-computer-name",
    "information-system-user-name",
    "information-system-domain",
    "information-system-uptime",
    "information-system-boot-time",
    "information-system-process-count",
    "information-system-thread-count",
    "information-system-handle-count",
    "information-system-handle-count-tooltip",
    "information-system-desktop",
    "information-system-windowing-system",
    "information-system-session-type",
    "information-system-virtual-terminal",
    "information-bios-firmware-type",
    "information-bios-uefi",
    "information-bios-legacy-bios",
    "information-bios-vendor",
    "information-bios-version",
    "information-bios-date",
    "information-bios-release",
    "information-bios-secure-boot",
    "information-bios-enabled",
    "information-bios-disabled",
    "information-board-manufacturer",
    "information-board-product",
    "information-board-version",
    "information-board-system-vendor",
    "information-board-system-product",
    "information-board-chassis-type",
    "information-board-serial-number",
    "information-board-serial-restricted-tooltip",
    "information-memory-total-physical",
    "information-memory-available-physical",
    "information-memory-memory-usage",
    "information-memory-committed",
    "information-memory-commit-limit",
    "information-memory-peak-committed-since-tasklens-started",
    "information-memory-page-size",
    "information-memory-huge-page-size",
    "information-memory-swap-total",
    "information-memory-zram-size",
    "information-memory-zram-none",
    "information-modules-module",
    "information-modules-slot",
    "information-modules-capacity",
    "information-modules-speed",
    "information-modules-type",
    "information-modules-form-factor",
    "information-modules-manufacturer",
    "information-modules-part-number",
    "information-modules-modules",
    "information-disk-model",
    "information-disk-type",
    "information-disk-interface",
    "information-disk-capacity",
    "information-disk-space",
    "information-disk-volumes",
    "information-disk-serial",
    "information-disk-ssd",
    "information-disk-hdd",
    "information-disk-volumes-none",
    "information-gpu-name",
    "information-gpu-driver-version",
    "information-gpu-kernel-module",
    "information-gpu-vulkan-version",
    "information-gpu-opengl-version",
    "information-gpu-temperature",
    "information-gpu-temperature-source",
    "information-gpu-dedicated-memory",
    "information-gpu-shared-memory",
    "information-gpu-resolution",
    "information-gpu-unknown-gpu",
    "information-net-connection",
    "information-net-details",
    "information-net-ip-address",
    "information-net-mac-address",
    "information-net-unknown-adapter",
    "information-net-ip-none",
];

/// The first label of each sub-block, in Memory modules, Disk drives, Graphics and Network
/// adapters order. The filter keeps a block's first row (UI-SYS-084).
pub const BLOCK_FIRST_LABELS: [&str; 4] = ["Module", "Model", "Name", "Connection"];

/// The highest SMBIOS chassis type with a word ("Stick PC").
const CHASSIS_MAX: usize = 36;
const KEY_CHASSIS_UNKNOWN: &str = "information-chassis-unknown";
const KEY_ARCH_BITS: &str = "information-os-architecture-bits";
const KEY_IDENTIFIER: &str = "information-cpu-identifier-value";
const KEY_FLAGS: &str = "information-cpu-flags-value";
const KEY_COMPUTER_NAME: &str = "information-system-computer-name-value";
const KEY_PAGE_SIZE: &str = "information-memory-page-size-value";
const KEY_MODULE_N: &str = "information-modules-module-value";
const KEY_SPEED_MTS: &str = "information-modules-speed-value";
const KEY_SPACE: &str = "information-disk-space-value";
const KEY_RESOLUTION: &str = "information-gpu-resolution-value";
const KEY_RESOLUTION_SIZE: &str = "information-gpu-resolution-size";
const KEY_PRIMARY: &str = "information-gpu-resolution-primary";
const KEY_LINK_SPEED: &str = "information-net-link-speed";
/// Icon sizes tried for a PNG distro logo, largest first.
const LOGO_PNG_SIZES: [&str; 7] = [
    "256x256", "128x128", "64x64", "48x48", "32x32", "24x24", "16x16",
];
/// Data directories when `XDG_DATA_DIRS` is not set (XDG Base Directory spec).
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// The labels and words of the five sections, translated once per strings version. A new table
/// is in en-US.
#[derive(Clone, Debug)]
pub struct InfoWords {
    /// `None` before the first refresh; then the strings version, `None` without strings.
    version: Option<Option<u64>>,
    /// In [`KEYS`] order.
    texts: [SharedString; KEYS.len()],
    /// Index 0 is "Unknown"; index n is SMBIOS chassis type n.
    chassis: [SharedString; CHASSIS_MAX + 1],
    /// "{machine} ({bits}-bit)".
    arch_bits: Template,
    /// "Family {f} Model {m} Stepping {s}".
    identifier: Template,
    /// "Virtualization: {v}, AVX-512: {a}".
    flags: Template,
    /// "{pretty} ({static})".
    computer_name: Template,
    /// "{bytes} bytes".
    page_size: Template,
    /// "Module {n}".
    module_n: Template,
    /// "{speed} MT/s".
    speed_mts: Template,
    /// "{free} free of {capacity}".
    space: Template,
    /// "{width}x{height} @ {hz} Hz".
    resolution: Template,
    /// "{width}x{height}", for a mode without a refresh rate.
    resolution_size: Template,
    /// "{mode} (primary)".
    primary: Template,
    /// "{speed} Mbps".
    link_speed: Template,
}

impl Default for InfoWords {
    fn default() -> Self {
        Self::build(|key| tr_en(key, &[]), Template::en)
    }
}

impl InfoWords {
    fn build(
        t: impl Fn(&str) -> SharedString,
        template: impl Fn(&str, &[&str]) -> Template,
    ) -> Self {
        Self {
            version: None,
            texts: KEYS.map(&t),
            chassis: std::array::from_fn(|n| {
                if n == 0 {
                    t(KEY_CHASSIS_UNKNOWN)
                } else {
                    t(&format!("information-chassis-{n}"))
                }
            }),
            arch_bits: template(KEY_ARCH_BITS, &["machine", "bits"]),
            identifier: template(KEY_IDENTIFIER, &["family", "model", "stepping"]),
            flags: template(KEY_FLAGS, &["virtualization", "avx512"]),
            computer_name: template(KEY_COMPUTER_NAME, &["pretty", "static"]),
            page_size: template(KEY_PAGE_SIZE, &["bytes"]),
            module_n: template(KEY_MODULE_N, &["n"]),
            speed_mts: template(KEY_SPEED_MTS, &["speed"]),
            space: template(KEY_SPACE, &["free", "capacity"]),
            resolution: template(KEY_RESOLUTION, &["width", "height", "hz"]),
            resolution_size: template(KEY_RESOLUTION_SIZE, &["width", "height"]),
            primary: template(KEY_PRIMARY, &["mode"]),
            link_speed: template(KEY_LINK_SPEED, &["speed"]),
        }
    }

    /// Translates every word when the strings changed since the last call. True when it did, so
    /// the caller rebuilds its rows.
    pub fn refresh(&mut self, cx: &App) -> bool {
        let version = Some(cx.try_global::<Strings>().map(|s| s.strings_version));
        if self.version == version {
            return false;
        }
        if version != Some(None) {
            *self = Self::build(|key| tr(cx, key), |key, args| Template::new(cx, key, args));
        }
        self.version = version;
        true
    }

    fn get(&self, w: W) -> &SharedString {
        &self.texts[w as usize]
    }

    /// The SMBIOS word for a chassis type; "Unknown" for a number without one.
    fn chassis(&self, n: u32) -> &SharedString {
        let n = usize::try_from(n)
            .ok()
            .filter(|n| (1..=CHASSIS_MAX).contains(n));
        &self.chassis[n.unwrap_or(0)]
    }
}

/// Operating System rows (UI-SYS-086).
pub fn build_os(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (os, w) = (&i.inv.os, i.words);
    push(out, w.get(W::OsName), text_value(&os.name), false);

    let version = if absent(os.version.validity, &os.version.value) {
        TlValue::new(w.get(W::OsRolling).clone())
    } else {
        text_value(&os.version)
    };
    push(out, w.get(W::OsVersion), version, false);
    push(out, w.get(W::OsBuild), text_value(&os.build), false);
    push(out, w.get(W::OsKernel), text_value(&os.kernel), false);
    push(
        out,
        w.get(W::OsKernelVersion),
        text_value(&os.kernel_version),
        false,
    );

    let arch = &os.architecture;
    let bits = &i.inv.processor.bits;
    let arch_value = if !shows(arch.validity) || arch.value.trim().is_empty() {
        text_value(arch)
    } else if shows(bits.validity) {
        let mut s = String::new();
        let _ = w.arch_bits.write(&mut s, &[&arch.value, &bits.value]);
        formatted(s, combined(&[arch.validity, bits.validity]))
    } else {
        text_value(arch)
    };
    push(out, w.get(W::OsArchitecture), arch_value, false);
    push(
        out,
        w.get(W::OsInstallDate),
        local_time_value(&os.install_date_s),
        false,
    );
    if !absent(os.package_manager.validity, &os.package_manager.value) {
        push(
            out,
            w.get(W::OsPackageManager),
            text_value(&os.package_manager),
            false,
        );
    }
    push(
        out,
        w.get(W::OsMachineId),
        text_value(&os.machine_id),
        false,
    );
}

/// Processor rows (UI-SYS-087).
pub fn build_processor(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (p, w) = (&i.inv.processor, i.words);
    let name = if absent(p.name.validity, &p.name.value) {
        TlValue::new(w.get(W::CpuUnknown).clone())
    } else {
        text_value(&p.name)
    };
    push(out, w.get(W::CpuName), name, false);
    push(out, w.get(W::CpuVendor), text_value(&p.vendor), false);
    push(
        out,
        w.get(W::CpuArchitecture),
        text_value(&p.architecture),
        false,
    );

    let validity = combined(&[p.family.validity, p.model.validity, p.stepping.validity]);
    let identifier = if shows(validity) {
        let mut s = String::new();
        let _ = w.identifier.write(
            &mut s,
            &[&p.family.value, &p.model.value, &p.stepping.value],
        );
        formatted(s, validity)
    } else {
        word_value(validity)
    };
    push(out, w.get(W::CpuIdentifier), identifier, false);
    push(
        out,
        w.get(W::CpuCores),
        num_value(&p.cores, |o, v| units::count(o, u64::from(v))),
        false,
    );
    push(
        out,
        w.get(W::CpuLogical),
        num_value(&p.logical, |o, v| units::count(o, u64::from(v))),
        false,
    );
    push(
        out,
        w.get(W::CpuBaseSpeed),
        num_value(&p.base_mhz, |o, v| units::mhz(o, v as f32)),
        false,
    );
    push(
        out,
        w.get(W::CpuMaxSpeed),
        num_value(&p.max_mhz, |o, v| units::mhz(o, v as f32)),
        false,
    );
    push(out, w.get(W::CpuMicrocode), text_value(&p.microcode), false);

    let validity = combined(&[p.virtualization.validity, p.avx512.validity]);
    let flags = if shows(validity) {
        let virt = match virtualization_kind(&p.virtualization.value) {
            Some(Virt::AmdV) => w.get(W::CpuYesAmdV),
            Some(Virt::VtX) => w.get(W::CpuYesVtX),
            None => w.get(W::CpuNo),
        };
        let avx = if p.avx512.value {
            w.get(W::CpuYes)
        } else {
            w.get(W::CpuNo)
        };
        let mut s = String::new();
        let _ = w.flags.write(&mut s, &[virt, avx]);
        formatted(s, validity)
    } else {
        word_value(validity)
    };
    push(out, w.get(W::CpuFlagsSummary), flags, false);
}

/// System rows (UI-SYS-088). System uptime and the three counts are live.
pub fn build_system(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (s, w) = (&i.inv.system, i.words);
    let host = &s.static_hostname;
    let pretty = &s.pretty_hostname;
    let pretty_text = pretty.value.trim();
    let computer = if shows(host.validity)
        && shows(pretty.validity)
        && !pretty_text.is_empty()
        && pretty_text != host.value.trim()
    {
        let mut t = String::new();
        let _ = w
            .computer_name
            .write(&mut t, &[&pretty_text, &host.value.trim()]);
        formatted(t, combined(&[host.validity, pretty.validity]))
    } else {
        text_value(host)
    };
    push(out, w.get(W::SysComputerName), computer, false);
    push(out, w.get(W::SysUserName), text_value(&s.user_name), false);
    if let Some(domain) = s
        .domain
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty() && *d != "(none)")
    {
        push(
            out,
            w.get(W::SysDomain),
            TlValue::new(SharedString::from(domain.to_owned())),
            false,
        );
    }

    push(out, w.get(W::SysUptime), uptime_value(i.sys), true);
    push(
        out,
        w.get(W::SysBootTime),
        local_time_value(&s.boot_time_s),
        false,
    );
    let cpu = &i.sys.cpu;
    push(
        out,
        w.get(W::SysProcessCount),
        metric_count(cpu.processes, u64::from),
        true,
    );
    push(
        out,
        w.get(W::SysThreadCount),
        metric_count(cpu.threads, u64::from),
        true,
    );
    let mut handles = row(
        w.get(W::SysHandleCount),
        metric_count(cpu.handles, |v| v),
        true,
    );
    handles.label_tooltip = Some(w.get(W::SysHandleCountTip).clone());
    out.push(handles);

    if !absent(s.desktop.validity, &s.desktop.value) {
        push(out, w.get(W::SysDesktop), text_value(&s.desktop), false);
    }
    push(
        out,
        w.get(W::SysWindowing),
        text_value(&s.windowing_system),
        false,
    );
    // `SystemInfo` holds one session string. R-205's "<Class> (<word>)" needs logind's Class
    // too, which the snapshot does not carry yet, so the string is shown as given.
    push(
        out,
        w.get(W::SysSessionType),
        text_value(&s.session_type),
        false,
    );
    if !absent(s.virtual_terminal.validity, &s.virtual_terminal.value) {
        push(
            out,
            w.get(W::SysVirtualTerminal),
            text_value(&s.virtual_terminal),
            false,
        );
    }
}

/// BIOS/Firmware rows (UI-SYS-092).
pub fn build_bios(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (f, w) = (&i.inv.firmware, i.words);
    let kind = &f.firmware_type;
    let kind_value = match (shows(kind.validity), kind.value) {
        (true, Some(FirmwareType::Uefi)) => formatted(w.get(W::BiosUefi), kind.validity),
        (true, Some(FirmwareType::LegacyBios)) => formatted(w.get(W::BiosLegacy), kind.validity),
        (true, None) => word_value(Validity::Unsupported),
        (false, _) => word_value(kind.validity),
    };
    push(out, w.get(W::BiosFirmwareType), kind_value, false);
    push(out, w.get(W::BiosVendor), text_value(&f.bios_vendor), false);
    push(
        out,
        w.get(W::BiosVersion),
        text_value(&f.bios_version),
        false,
    );
    push(out, w.get(W::BiosDate), text_value(&f.bios_date), false);
    push(
        out,
        w.get(W::BiosRelease),
        text_value(&f.bios_release),
        false,
    );
    let secure = &f.secure_boot;
    let secure_value = if shows(secure.validity) {
        let word = if secure.value {
            W::BiosEnabled
        } else {
            W::BiosDisabled
        };
        formatted(w.get(word), secure.validity)
    } else {
        word_value(secure.validity)
    };
    push(out, w.get(W::BiosSecureBoot), secure_value, false);
}

/// Motherboard rows (UI-SYS-093).
pub fn build_motherboard(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (b, w) = (&i.inv.board, i.words);
    push(
        out,
        w.get(W::BoardManufacturer),
        text_value(&b.manufacturer),
        false,
    );
    push(out, w.get(W::BoardProduct), text_value(&b.product), false);
    push(out, w.get(W::BoardVersion), text_value(&b.version), false);
    push(
        out,
        w.get(W::BoardSystemVendor),
        text_value(&b.system_vendor),
        false,
    );
    push(
        out,
        w.get(W::BoardSystemProduct),
        text_value(&b.system_product),
        false,
    );
    let chassis = &b.chassis_type;
    let chassis_value = if shows(chassis.validity) {
        // The DMI `chassis_type` number; anything that is not a number reads "Unknown".
        let n = chassis.value.trim().parse().unwrap_or(0);
        formatted(w.chassis(n), chassis.validity)
    } else {
        word_value(chassis.validity)
    };
    push(out, w.get(W::BoardChassisType), chassis_value, false);
    let mut serial = text_value(&b.serial);
    if serial.validity == Validity::Restricted {
        // PERF-102: board_serial is mode 0400 and the helper does not read it.
        serial.tooltip = Some(w.get(W::BoardSerialRestrictedTip).clone());
    }
    push(out, w.get(W::BoardSerial), serial, false);
}

/// Memory rows (UI-SYS-089). Available physical, Memory usage, Committed and Peak committed are
/// live.
pub fn build_memory(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let (m, live, w) = (&i.inv.memory, &i.sys.memory, i.words);
    push(out, w.get(W::MemTotal), bytes_value(&m.total_bytes), false);
    let validity = live.validity;
    push(
        out,
        w.get(W::MemAvailable),
        live_value(validity, |s| units::bytes(s, live.available)),
        true,
    );
    let usage = if shows(validity) && live.total == 0 {
        word_value(Validity::Unsupported)
    } else {
        live_value(validity, |s| {
            units::percent(s, (live.used as f64 / live.total as f64 * 100.) as f32);
        })
    };
    push(out, w.get(W::MemUsage), usage, true);
    push(
        out,
        w.get(W::MemCommitted),
        live_value(validity, |s| units::bytes(s, live.committed)),
        true,
    );
    push(
        out,
        w.get(W::MemCommitLimit),
        live_value(validity, |s| units::bytes(s, live.commit_limit)),
        false,
    );
    push(
        out,
        w.get(W::MemPeak),
        bytes_value(&m.peak_committed_bytes),
        true,
    );
    push(
        out,
        w.get(W::MemPageSize),
        num_value(&m.page_size, |s, v| {
            let _ = w.page_size.write(s, &[&v]);
        }),
        false,
    );
    push(
        out,
        w.get(W::MemHugePageSize),
        bytes_value(&m.huge_page_size),
        false,
    );
    push(
        out,
        w.get(W::MemSwapTotal),
        bytes_value(&m.swap_total_bytes),
        false,
    );
    let zram = &m.zram_bytes;
    // The worker sums `disksize` over the zram devices, so 0 means there is none.
    let none = zram.validity == Validity::Unsupported || (shows(zram.validity) && zram.value == 0);
    let zram_value = if none {
        TlValue::new(w.get(W::MemZramNone).clone())
    } else {
        bytes_value(zram)
    };
    push(out, w.get(W::MemZram), zram_value, false);
}

/// Memory modules rows (UI-SYS-090): one block per module in slot order. No row holds a module
/// serial (D-007): `MemoryModule` has no serial field.
pub fn build_modules(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let w = i.words;
    if i.inv.modules.is_empty() {
        push(
            out,
            w.get(W::ModNone),
            word_value(Validity::Unsupported),
            false,
        );
        return;
    }
    for (n, m) in i.inv.modules.iter().enumerate() {
        let block = block_number(n);
        let mut title = String::new();
        let _ = w.module_n.write(&mut title, &[&(n + 1)]);
        push_in(out, block, w.get(W::ModModule), TlValue::new(title), false);
        push_in(out, block, w.get(W::ModSlot), text_value(&m.slot), false);
        push_in(
            out,
            block,
            w.get(W::ModCapacity),
            bytes_value(&m.capacity_bytes),
            false,
        );
        let speed = num_value(&m.speed_mts, |s, v| {
            let _ = w.speed_mts.write(s, &[&v]);
        });
        push_in(out, block, w.get(W::ModSpeed), speed, false);
        push_in(out, block, w.get(W::ModType), text_value(&m.kind), false);
        push_in(
            out,
            block,
            w.get(W::ModFormFactor),
            text_value(&m.form_factor),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::ModManufacturer),
            text_value(&m.manufacturer),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::ModPartNumber),
            text_value(&m.part_number),
            false,
        );
    }
}

/// Disk drives rows (UI-SYS-091): one block per disk in Performance sidebar order, which is
/// device index order; a disk the reducer has not matched goes last. Space is live.
pub fn build_disks(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let w = i.words;
    let mut disks: Vec<_> = i.inv.disks.iter().collect();
    disks.sort_by_key(|d| d.device.map_or((1, DeviceIdx(0)), |idx| (0, idx)));
    for (n, d) in disks.into_iter().enumerate() {
        let block = block_number(n);
        push_in(out, block, w.get(W::DiskModel), text_value(&d.model), false);
        let kind = &d.kind;
        let kind_value = if shows(kind.validity) {
            // The worker's PERF-047 kind word. Of those, only HDD rotates.
            match kind.value.trim() {
                "HDD" => formatted(w.get(W::DiskHdd), kind.validity),
                "NVMe" | "SSD" | "eMMC" | "SD" => formatted(w.get(W::DiskSsd), kind.validity),
                _ => text_value(kind),
            }
        } else {
            word_value(kind.validity)
        };
        push_in(out, block, w.get(W::DiskType), kind_value, false);
        push_in(
            out,
            block,
            w.get(W::DiskInterface),
            text_value(&d.interface),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::DiskCapacity),
            bytes_value(&d.capacity_bytes),
            false,
        );
        let validity = combined(&[d.free_bytes.validity, d.capacity_bytes.validity]);
        let space = if shows(validity) {
            let mut s = String::new();
            w.space.write_each(&mut s, |arg, s| match arg {
                0 => units::bytes(s, d.free_bytes.value),
                _ => units::bytes(s, d.capacity_bytes.value),
            });
            formatted(s, validity)
        } else {
            word_value(validity)
        };
        push_in(out, block, w.get(W::DiskSpace), space, true);
        let volumes = if d.volumes.is_empty() {
            TlValue::new(w.get(W::DiskVolumesNone).clone())
        } else {
            TlValue::new(d.volumes.join(", "))
        };
        push_in(out, block, w.get(W::DiskVolumes), volumes, false);
        push_in(
            out,
            block,
            w.get(W::DiskSerial),
            text_value(&d.serial),
            false,
        );
    }
}

/// Graphics rows (UI-SYS-094): one block per GPU. Temperature is live. Resolution has one line
/// per display.
pub fn build_graphics(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let w = i.words;
    for (n, g) in i.inv.graphics.iter().enumerate() {
        let block = block_number(n);
        let live = g
            .device
            .and_then(|idx| i.sys.gpus.iter().find(|(d, _)| *d == idx))
            .map(|(_, t)| t);
        let name = if absent(g.name.validity, &g.name.value) {
            TlValue::new(w.get(W::GpuUnknown).clone())
        } else {
            text_value(&g.name)
        };
        push_in(out, block, w.get(W::GpuName), name, false);
        push_in(
            out,
            block,
            w.get(W::GpuDriverVersion),
            text_value(&g.driver_version),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::GpuKernelModule),
            text_value(&g.kernel_module),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::GpuVulkan),
            text_value(&g.vulkan_version),
            false,
        );
        push_in(
            out,
            block,
            w.get(W::GpuOpenGl),
            text_value(&g.opengl_version),
            false,
        );
        let temp = match live {
            Some(t) => live_value(t.temp_c.validity, |s| units::celsius(s, t.temp_c.value)),
            None => word_value(Validity::Unsupported),
        };
        push_in(out, block, w.get(W::GpuTemperature), temp, true);
        push_in(
            out,
            block,
            w.get(W::GpuTemperatureSource),
            text_value(&g.temperature_source),
            false,
        );
        // The worker leaves NVIDIA sizes to NVML, which the live GPU totals carry (PERF-064).
        let dedicated = sized(&g.dedicated_bytes, live.map(|t| t.mem_total));
        push_in(out, block, w.get(W::GpuDedicated), dedicated, false);
        let shared = sized(&g.shared_bytes, live.map(|t| t.shared_total));
        push_in(out, block, w.get(W::GpuShared), shared, false);
        let resolution = resolution_value(w, &g.displays);
        push_in(out, block, w.get(W::GpuResolution), resolution, false);
    }
}

/// Network adapters rows (UI-SYS-095) without the Public IP row: one block per interface the
/// worker kept by the Performance sidebar rules (PERF-004). IP address has one line per address,
/// in snapshot order.
pub fn build_network(i: &InfoInputs, out: &mut Vec<InfoRow>) {
    let w = i.words;
    for (n, a) in i.inv.network.iter().enumerate() {
        let block = block_number(n);
        let connection = if absent(a.connection.validity, &a.connection.value) {
            TlValue::new(w.get(W::NetUnknown).clone())
        } else {
            text_value(&a.connection)
        };
        push_in(out, block, w.get(W::NetConnection), connection, false);
        push_in(out, block, w.get(W::NetDetails), details_value(w, a), false);
        let ips = if a.ip_addresses.is_empty() {
            TlValue::new(w.get(W::NetIpNone).clone())
        } else {
            TlValue::new(a.ip_addresses.join("\n"))
        };
        push_in(out, block, w.get(W::NetIp), ips, false);
        push_in(out, block, w.get(W::NetMac), text_value(&a.mac), false);
    }
}

/// "<adapter name>, <driver>, <speed> Mbps" from the parts that show; "Not reported" when none
/// does.
fn details_value(w: &InfoWords, a: &NetworkAdapter) -> TlValue {
    let mut s = String::new();
    let mut parts: SmallVec<[Validity; 3]> = SmallVec::new();
    for f in [&a.adapter_name, &a.driver] {
        if shows(f.validity) && !f.value.trim().is_empty() {
            if !parts.is_empty() {
                s.push_str(", ");
            }
            parts.push(f.validity);
            s.push_str(f.value.trim());
        }
    }
    let speed = &a.link_speed_mbps;
    if shows(speed.validity) && speed.value > 0 {
        if !parts.is_empty() {
            s.push_str(", ");
        }
        parts.push(speed.validity);
        let _ = w.link_speed.write(&mut s, &[&speed.value]);
    }
    if parts.is_empty() {
        return word_value(Validity::Unsupported);
    }
    formatted(s, combined(&parts))
}

/// One line per display, with " (primary)" on the primary one; "Not reported" with none.
fn resolution_value(w: &InfoWords, displays: &[DisplayMode]) -> TlValue {
    if displays.is_empty() {
        return word_value(Validity::Unsupported);
    }
    let mut s = String::new();
    let mut mode = String::new();
    for (n, d) in displays.iter().enumerate() {
        if n > 0 {
            s.push('\n');
        }
        mode.clear();
        // Sysfs has no refresh rate, so a mode can come without one.
        let hz = d.refresh_mhz.saturating_add(500) / 1000;
        if hz == 0 {
            let _ = w.resolution_size.write(&mut mode, &[&d.width, &d.height]);
        } else {
            let _ = w.resolution.write(&mut mode, &[&d.width, &d.height, &hz]);
        }
        if d.primary {
            let _ = w.primary.write(&mut s, &[&mode]);
        } else {
            s.push_str(&mode);
        }
    }
    TlValue::new(s)
}

/// The inventory size, else the live one while the inventory has none.
fn sized(f: &InvField<u64>, live: Option<Metric<u64>>) -> TlValue {
    match live {
        Some(m) if !shows(f.validity) && shows(m.validity) => {
            live_value(m.validity, |s| units::bytes(s, m.value))
        }
        _ => bytes_value(f),
    }
}

/// Sub-block `n`, counted from 0, as a block number from 1.
fn block_number(n: usize) -> u8 {
    u8::try_from(n + 1).unwrap_or(u8::MAX)
}

/// An IEC size, "62.70 GiB".
fn bytes_value(f: &InvField<u64>) -> TlValue {
    num_value(f, units::bytes)
}

/// A live value under the validity of its whole reading.
fn live_value(v: Validity, write: impl FnOnce(&mut String)) -> TlValue {
    if !shows(v) {
        return word_value(v);
    }
    let mut s = String::new();
    write(&mut s);
    formatted(s, v)
}

/// The os-release LOGO icon as a file path, or `None` when the icon is not found in the hicolor
/// theme or `pixmaps` of the XDG data directories.
pub fn os_logo(i: &InfoInputs) -> Option<SharedString> {
    let name = i.inv.os.logo_icon.as_deref()?;
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_DATA_DIRS.to_owned());
    let roots: Vec<&Path> = dirs
        .split(':')
        .filter(|d| !d.is_empty())
        .map(Path::new)
        .collect();
    find_logo(name, &roots).map(|p| SharedString::from(p.to_string_lossy().into_owned()))
}

/// The first existing file for icon `name` under `roots`: hicolor scalable SVG, hicolor PNG
/// from 256 down to 16, then `pixmaps` SVG and PNG. A name with a path separator or a leading
/// "." is rejected, since os-release is not trusted to name files.
pub fn find_logo(name: &str, roots: &[&Path]) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\0']) {
        return None;
    }
    roots.iter().find_map(|root| {
        let hicolor = root.join("icons/hicolor");
        std::iter::once(hicolor.join(format!("scalable/apps/{name}.svg")))
            .chain(
                LOGO_PNG_SIZES
                    .iter()
                    .map(|size| hicolor.join(format!("{size}/apps/{name}.png"))),
            )
            .chain([
                root.join(format!("pixmaps/{name}.svg")),
                root.join(format!("pixmaps/{name}.png")),
            ])
            .find(|p| p.is_file())
    })
}

fn row(label: &SharedString, value: TlValue, live: bool) -> InfoRow {
    InfoRow {
        label: label.clone(),
        value,
        live,
        block: 0,
        label_tooltip: None,
    }
}

fn push(out: &mut Vec<InfoRow>, label: &SharedString, value: TlValue, live: bool) {
    out.push(row(label, value, live));
}

/// [`push`] into sub-block `block`.
fn push_in(out: &mut Vec<InfoRow>, block: u8, label: &SharedString, value: TlValue, live: bool) {
    out.push(InfoRow {
        block,
        ..row(label, value, live)
    });
}

/// The grid shows the text for these; for the others it shows the word.
fn shows(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

/// The first validity that does not show a value; else Stale when any is Stale; else Valid.
fn combined(all: &[Validity]) -> Validity {
    all.iter()
        .copied()
        .find(|v| !shows(*v))
        .or_else(|| all.iter().copied().find(|v| *v == Validity::Stale))
        .unwrap_or(Validity::Valid)
}

/// A row that is hidden when its value does not exist: Unsupported, Missing or empty.
fn absent(v: Validity, text: &str) -> bool {
    matches!(v, Validity::Unsupported | Validity::Missing) || (shows(v) && text.trim().is_empty())
}

/// The word for `v`, with "Not reported" for Unsupported (R-050).
fn word_value(v: Validity) -> TlValue {
    TlValue {
        validity: v,
        word: (v == Validity::Unsupported).then_some(UnsupportedWord::NotReported),
        ..TlValue::new(SharedString::default())
    }
}

/// A formatted text with the validity of its source (Valid or Stale).
fn formatted(text: impl Into<SharedString>, v: Validity) -> TlValue {
    TlValue {
        validity: v,
        ..TlValue::new(text)
    }
}

/// A text field: the text, or "Not reported" when it is empty.
fn text_value(f: &InvField<Arc<str>>) -> TlValue {
    if !shows(f.validity) {
        return word_value(f.validity);
    }
    if f.value.trim().is_empty() {
        return word_value(Validity::Unsupported);
    }
    formatted(SharedString::from(f.value.clone()), f.validity)
}

fn num_value<T: Copy>(f: &InvField<T>, write: impl FnOnce(&mut String, T)) -> TlValue {
    if !shows(f.validity) {
        return word_value(f.validity);
    }
    let mut s = String::new();
    write(&mut s, f.value);
    formatted(s, f.validity)
}

/// A live count grouped with U+2009 (table d).
fn metric_count<T: Copy + Default>(m: Metric<T>, to_u64: impl FnOnce(T) -> u64) -> TlValue {
    if !shows(m.validity) {
        return word_value(m.validity);
    }
    let mut s = String::new();
    units::count(&mut s, to_u64(m.value));
    formatted(s, m.validity)
}

/// CLOCK_BOOTTIME at the snapshot is the up time (PERF-034).
fn uptime_value(sys: &SystemSnapshot) -> TlValue {
    let at = sys.header.observed_end;
    if at == 0 {
        return word_value(Validity::WarmingUp);
    }
    let validity = if sys.header.validity == Validity::Stale {
        Validity::Stale
    } else {
        Validity::Valid
    };
    let mut s = String::new();
    units::uptime(&mut s, at / 1_000_000_000);
    formatted(s, validity)
}

/// Unix seconds as local "YYYY-MM-DD HH:MM:SS"; "Not reported" when the time does not convert.
fn local_time_value(f: &InvField<i64>) -> TlValue {
    if !shows(f.validity) {
        return word_value(f.validity);
    }
    let mut s = String::new();
    if !write_local(&mut s, f.value) {
        return word_value(Validity::Unsupported);
    }
    formatted(s, f.validity)
}

/// Writes `unix_s` in the system time zone. False when it is out of range.
fn write_local(out: &mut String, unix_s: i64) -> bool {
    let Ok(ts) = jiff::Timestamp::from_second(unix_s) else {
        return false;
    };
    let z = ts.to_zoned(jiff::tz::TimeZone::system());
    units::date_time(
        out,
        units::LocalDateTime {
            year: i32::from(z.year()),
            month: z.month() as u8,
            day: z.day() as u8,
            hour: z.hour() as u8,
            minute: z.minute() as u8,
            second: z.second() as u8,
        },
    );
    true
}

enum Virt {
    AmdV,
    VtX,
}

/// `ProcessorInfo.virtualization` holds the CPU flag: `svm` is AMD-V and `vmx` is VT-x. The
/// technology names are accepted too.
fn virtualization_kind(flag: &str) -> Option<Virt> {
    let mut words = flag.split([' ', ',']).map(str::trim);
    words.find_map(|f| {
        if f.eq_ignore_ascii_case("svm") || f.eq_ignore_ascii_case("AMD-V") {
            Some(Virt::AmdV)
        } else if f.eq_ignore_ascii_case("vmx") || f.eq_ignore_ascii_case("VT-x") {
            Some(Virt::VtX)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_words() {
        assert_eq!(KEYS[W::OsName as usize], "information-os-name");
        assert_eq!(KEYS[W::CpuName as usize], "information-cpu-name");
        assert_eq!(
            KEYS[W::SysComputerName as usize],
            "information-system-computer-name"
        );
        assert_eq!(
            KEYS[W::BiosFirmwareType as usize],
            "information-bios-firmware-type"
        );
        assert_eq!(
            KEYS[W::BoardSerialRestrictedTip as usize],
            "information-board-serial-restricted-tooltip"
        );
        let words = InfoWords::default();
        assert!(
            words.texts.iter().all(|t| !t.starts_with("information-")),
            "every key has a message"
        );
        assert_eq!(words.get(W::SysHandleCount).as_ref(), "Handle count");
        assert_eq!(words.chassis(36).as_ref(), "Stick PC");
    }

    #[test]
    fn combined_validity() {
        use Validity::*;
        assert_eq!(combined(&[Valid, Valid]), Valid);
        assert_eq!(combined(&[Valid, Stale]), Stale);
        assert_eq!(combined(&[Stale, Restricted]), Restricted);
    }

    #[test]
    fn virtualization_words() {
        assert!(matches!(virtualization_kind("svm"), Some(Virt::AmdV)));
        assert!(matches!(virtualization_kind("vmx"), Some(Virt::VtX)));
        assert!(virtualization_kind("").is_none());
    }
}
