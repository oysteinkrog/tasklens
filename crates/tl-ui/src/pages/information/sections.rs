//! Row builders for the Operating System, Processor, System, BIOS/Firmware and Motherboard
//! sections (UI-SYS-086 to UI-SYS-088, UI-SYS-092, UI-SYS-093; PERF-095 to PERF-097, PERF-101,
//! PERF-102, APP-041, R-050, table d).
//!
//! Each builder turns the snapshot into [`InfoRow`]s with fixed labels, formats and hide rules,
//! so the section entity only draws. The page calls them only when INVENTORY or SYSTEM changes;
//! they append to the caller's `Vec` and format each value once.
//!
//! A value whose field is not Valid or Stale carries that validity and no text; the grid shows
//! the word (UI-SYS-082). Unsupported inventory fields read "Not reported" (R-050), and so does
//! a Valid field with empty text. Values are local machine data and are never logged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, SharedString};
use tl_model::devices::DeviceCatalog;
use tl_model::snapshot::{FirmwareType, InvField, InventorySnapshot, SystemSnapshot};
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
}

const KEYS: [&str; 56] = [
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
];

/// The highest SMBIOS chassis type with a word ("Stick PC").
const CHASSIS_MAX: usize = 36;
const KEY_CHASSIS_UNKNOWN: &str = "information-chassis-unknown";
const KEY_ARCH_BITS: &str = "information-os-architecture-bits";
const KEY_IDENTIFIER: &str = "information-cpu-identifier-value";
const KEY_FLAGS: &str = "information-cpu-flags-value";
const KEY_COMPUTER_NAME: &str = "information-system-computer-name-value";
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
