//! The CPU page's Details grid (docs/ui/performance.md UI-PRF-024; PERF-033 to PERF-040).
//!
//! Rows 1 to 12 are live and update at 1 Hz from `SystemSnapshot`; rows 13 to 26 are static and
//! update from `CpuStaticInfo`. Row 10 (Power draw) is live, but `CpuStaticInfo.rapl` decides
//! whether it is shown: it stays hidden until the static info reports a RAPL zone (PERF-013).
//! Each value is formatted only when its bits or text change, and tooltips are compared in place
//! before they are rebuilt, so a tick where nothing changed does not allocate.
//!
//! The tooltip texts are `perf-cpu-tip-*` in `en-US.ftl`, translated by the model with its
//! fallback words ([`KeyValueModel::with_texts`]).

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use gpui_kit::SharedString;
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{
    CacheLevelInfo, CpuStaticInfo, InvField, RaplState, SecurityFeature, SystemSnapshot, VirtState,
};
use tl_model::system::Metric;
use tl_model::units;
use tl_model::validity::Validity;

use super::details::{Fallback, KeyValueModel};

pub const ROW_UTILIZATION: &str = "perf-cpu-row-utilization";
pub const ROW_KERNEL_TIME: &str = "perf-cpu-row-kernel-time";
pub const ROW_SPEED: &str = "perf-cpu-row-speed";
pub const ROW_PROCESSES: &str = "perf-cpu-row-processes";
pub const ROW_THREADS: &str = "perf-cpu-row-threads";
pub const ROW_HANDLES: &str = "perf-cpu-row-handles";
pub const ROW_UP_TIME: &str = "perf-cpu-row-up-time";
pub const ROW_BASE_SPEED: &str = "perf-cpu-row-base-speed";
pub const ROW_SOCKETS: &str = "perf-cpu-row-sockets";
pub const ROW_POWER: &str = "perf-cpu-row-power";
pub const ROW_TEMPERATURE: &str = "perf-cpu-row-temperature";
pub const ROW_LOAD_AVERAGE: &str = "perf-cpu-row-load-average";
pub const ROW_CORES: &str = "perf-cpu-row-cores";
pub const ROW_LOGICAL: &str = "perf-cpu-row-logical-processors";
pub const ROW_VIRTUALIZATION: &str = "perf-cpu-row-virtualization";
pub const ROW_RUNNING_IN_VM: &str = "perf-cpu-row-running-in-vm";
pub const ROW_SECURITY: &str = "perf-cpu-row-security";
pub const ROW_HYPERVISOR: &str = "perf-cpu-row-hypervisor";
pub const ROW_L1: &str = "perf-cpu-row-l1";
pub const ROW_L2: &str = "perf-cpu-row-l2";
pub const ROW_L3: &str = "perf-cpu-row-l3";
pub const ROW_CPUFREQ_DRIVER: &str = "perf-cpu-row-cpufreq-driver";
pub const ROW_GOVERNOR: &str = "perf-cpu-row-governor";
pub const ROW_EPP: &str = "perf-cpu-row-epp";
pub const ROW_PLATFORM_PROFILE: &str = "perf-cpu-row-platform-profile";
pub const ROW_MICROCODE: &str = "perf-cpu-row-microcode";

/// The 26 rows in grid order.
pub const ROWS: [&str; 26] = [
    ROW_UTILIZATION,
    ROW_KERNEL_TIME,
    ROW_SPEED,
    ROW_PROCESSES,
    ROW_THREADS,
    ROW_HANDLES,
    ROW_UP_TIME,
    ROW_BASE_SPEED,
    ROW_SOCKETS,
    ROW_POWER,
    ROW_TEMPERATURE,
    ROW_LOAD_AVERAGE,
    ROW_CORES,
    ROW_LOGICAL,
    ROW_VIRTUALIZATION,
    ROW_RUNNING_IN_VM,
    ROW_SECURITY,
    ROW_HYPERVISOR,
    ROW_L1,
    ROW_L2,
    ROW_L3,
    ROW_CPUFREQ_DRIVER,
    ROW_GOVERNOR,
    ROW_EPP,
    ROW_PLATFORM_PROFILE,
    ROW_MICROCODE,
];

/// `{$mhz}`: the fastest core's MHz.
const TIP_FASTEST: &str = "perf-cpu-tip-fastest";
const TIP_BOGOMIPS: &str = "perf-cpu-tip-bogomips";
const TIP_HANDLES: &str = "perf-cpu-tip-handles";
/// `{$time}`: the boot time.
const TIP_BOOT: &str = "perf-cpu-tip-boot";
const TIP_LOADAVG: &str = "perf-cpu-tip-loadavg";
/// `{$size}`: the L1 data caches, for example "16 × 32 KiB".
const TIP_L1_DATA: &str = "perf-cpu-tip-l1-data";
/// `{$size}`: the L1 instruction caches.
const TIP_L1_INSTRUCTION: &str = "perf-cpu-tip-l1-instruction";
/// `{$size}`: the L4 total.
const TIP_L4: &str = "perf-cpu-tip-l4";
/// A boot time that does not convert to a date.
const TIME_UNKNOWN: &str = "perf-cpu-time-unknown";
/// `{$pkg}` and `{$core}`: the RAPL watts with one decimal.
const TIP_POWER: &str = "perf-cpu-tip-power";
const TIP_VULNS: &str = "perf-cpu-tip-vulns";
/// No security feature, and no hypervisor.
const FEATURE_NONE: &str = "perf-cpu-feature-none";
const VIRT_ENABLED: &str = "perf-cpu-virt-enabled";
const VIRT_NO_KVM: &str = "perf-cpu-virt-no-kvm";
const VIRT_NOT_SUPPORTED: &str = "perf-cpu-virt-not-supported";
/// Running in VM on bare metal.
const VM_NO: &str = "perf-cpu-vm-no";
const FEATURE_SECURE_BOOT: &str = "perf-cpu-feature-secure-boot";
const FEATURE_LOCKDOWN: &str = "perf-cpu-feature-lockdown";
const FEATURE_MEMORY_ENCRYPTION: &str = "perf-cpu-feature-memory-encryption";
const FEATURE_SHADOW_STACK: &str = "perf-cpu-feature-shadow-stack";
const FEATURE_IOMMU: &str = "perf-cpu-feature-iommu";
/// The tooltip texts and value words, and their arguments.
const TEXTS: [(&str, &[&str]); 22] = [
    (TIP_FASTEST, &["mhz"]),
    (TIP_BOGOMIPS, &[]),
    (TIP_HANDLES, &[]),
    (TIP_BOOT, &["time"]),
    (TIP_LOADAVG, &[]),
    (TIP_L1_DATA, &["size"]),
    (TIP_L1_INSTRUCTION, &["size"]),
    (TIP_L4, &["size"]),
    (TIME_UNKNOWN, &[]),
    (TIP_POWER, &["pkg", "core"]),
    (TIP_VULNS, &["mitigated", "vulnerable", "not_affected"]),
    (FEATURE_NONE, &[]),
    (VIRT_ENABLED, &[]),
    (VIRT_NO_KVM, &[]),
    (VIRT_NOT_SUPPORTED, &[]),
    (VM_NO, &[]),
    (FEATURE_SECURE_BOOT, &[]),
    (FEATURE_LOCKDOWN, &[]),
    (FEATURE_MEMORY_ENCRYPTION, &[]),
    (FEATURE_SHADOW_STACK, &[]),
    (FEATURE_IOMMU, &[]),
];
/// Separates the active security features.
const LIST_SEP: &str = ", ";
/// Marks a clock read from BogoMIPS.
const BOGOMIPS_MARK: &str = " (?)";
/// Separates the parts of a tooltip.
const DOT: &str = " · ";

/// hwmon chips that measure the CPU, in the order PERF-039 searches them.
const CPU_CHIPS: [&str; 4] = ["k10temp", "zenpower", "coretemp", "cpu_thermal"];

/// One temperature sensor of the CPU hwmon chip, for the Temperature tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuSensor {
    pub idx: DeviceIdx,
    /// hwmon label, for example `Tctl`.
    pub label: Arc<str>,
}

/// What the live rows need beyond `SystemSnapshot`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuLiveInputs<'a> {
    /// CLOCK_BOOTTIME now: the up time, and the age of a Stale value.
    pub now_boot_ns: u64,
    /// `SourceHealth.last_error` of the CPU source, shown for Failed values.
    pub cpu_error: Option<&'a str>,
    /// From [`cpu_temp_sensors`].
    pub temp_sensors: &'a [CpuSensor],
}

/// A Details model with the 26 rows, collecting, and the tooltip texts. Power draw starts
/// hidden until [`update_static`] sees a RAPL zone.
pub fn cpu_details_model() -> KeyValueModel {
    let mut model = KeyValueModel::new(&ROWS).with_texts(&TEXTS);
    model.hide(ROW_POWER);
    model
}

/// Fills `out` with the temperature sensors of the first CPU hwmon chip present, in catalog
/// order. Run on catalog changes only.
pub fn cpu_temp_sensors(catalog: &DeviceCatalog, out: &mut Vec<CpuSensor>) {
    out.clear();
    let sensor = |want: &'static str| {
        catalog
            .entries
            .iter()
            .filter_map(move |e| match &e.identity {
                DeviceIdentity::Sensor { chip, label } if e.present && &**chip == want => {
                    Some(CpuSensor {
                        idx: e.idx,
                        label: label.clone(),
                    })
                }
                _ => None,
            })
    };
    if let Some(chip) = CPU_CHIPS.iter().find(|c| sensor(c).next().is_some()) {
        out.extend(sensor(chip));
    }
}

/// Updates rows 1 to 12. True when any shown text or state changed.
pub fn update_live(model: &mut KeyValueModel, sys: &SystemSnapshot, inp: &CpuLiveInputs) -> bool {
    let cpu = &sys.cpu;
    let age_ns = inp.now_boot_ns.saturating_sub(sys.header.observed_end);
    let live = Live {
        age_ns,
        error: inp.cpu_error,
    };
    let mut changed = false;

    changed |= live.row(model, ROW_UTILIZATION, cpu.util_pct, f32_bits, |o, v| {
        units::percent(o, v)
    });
    changed |= live.row(model, ROW_KERNEL_TIME, cpu.kernel_pct, f32_bits, |o, v| {
        units::percent(o, v)
    });

    let bogomips = cpu.freq_is_bogomips;
    let speed_bits = |v: u32| u64::from(v) | u64::from(bogomips) << 32;
    changed |= live.row(model, ROW_SPEED, cpu.freq_mhz, speed_bits, |o, v| {
        units::mhz(o, v as f32);
        if bogomips {
            o.push_str(BOGOMIPS_MARK);
        }
    });
    if shows_value(cpu.freq_mhz.validity) {
        let fastest = cpu.freq_fastest_mhz;
        changed |= if bogomips {
            let tip = model.text(TIP_BOGOMIPS);
            set_tip(model, ROW_SPEED, |o| tip.write(o, &[]))
        } else if shows_value(fastest.validity) {
            let tip = model.text(TIP_FASTEST);
            set_tip(model, ROW_SPEED, |o| tip.write(o, &[&fastest.value]))
        } else {
            clear_tip(model, ROW_SPEED)
        };
    }

    changed |= live.row(model, ROW_PROCESSES, cpu.processes, u64::from, |o, v| {
        units::count(o, u64::from(v))
    });
    changed |= live.row(model, ROW_THREADS, cpu.threads, u64::from, |o, v| {
        units::count(o, u64::from(v))
    });
    changed |= live.row(model, ROW_HANDLES, cpu.handles, |v| v, units::count);
    if shows_value(cpu.handles.validity) {
        let tip = model.text(TIP_HANDLES);
        changed |= set_tip(model, ROW_HANDLES, |o| tip.write(o, &[]));
    }

    // CLOCK_BOOTTIME is the up time, so this row is always Valid.
    let up_s = inp.now_boot_ns / 1_000_000_000;
    changed |= model.set_valid(ROW_UP_TIME, up_s, |o| units::uptime(o, up_s));
    // btime does not change within a boot, so the tooltip changes only with the language.
    if sys.boot_time_s > 0 {
        let tip = model.text(TIP_BOOT);
        let unknown = model.text(TIME_UNKNOWN);
        let boot = LocalTime(sys.boot_time_s, unknown.text());
        changed |= set_tip(model, ROW_UP_TIME, |o| tip.write(o, &[&boot]));
    }

    changed |= live.row(model, ROW_BASE_SPEED, cpu.base_mhz, u64::from, |o, v| {
        units::mhz(o, v as f32)
    });
    changed |= live.row(model, ROW_SOCKETS, cpu.sockets, u64::from, |o, v| {
        units::count(o, u64::from(v))
    });

    changed |= update_power(model, sys, &live);
    changed |= update_temperature(model, sys, &live, inp.temp_sensors);

    changed |= live.row(model, ROW_LOAD_AVERAGE, cpu.load_avg, load_bits, |o, _| {
        // Formatted from the bits, so the text and the bits always agree.
        write_load(o, load_bits(cpu.load_avg.value));
    });
    if shows_value(cpu.load_avg.validity) {
        let tip = model.text(TIP_LOADAVG);
        changed |= set_tip(model, ROW_LOAD_AVERAGE, |o| tip.write(o, &[]));
    }
    changed
}

/// Updates rows 13 to 26, and shows or hides row 10. True when any shown text or state changed.
pub fn update_static(model: &mut KeyValueModel, info: &CpuStaticInfo) -> bool {
    let mut changed = false;
    // Zero cores means the static info has not arrived yet: the rows keep collecting.
    if info.cores > 0 {
        changed |= model.set_valid(ROW_CORES, u64::from(info.cores), |o| {
            units::count(o, u64::from(info.cores))
        });
    }
    if info.logical > 0 {
        changed |= model.set_valid(ROW_LOGICAL, u64::from(info.logical), |o| {
            units::count(o, u64::from(info.logical))
        });
    }
    if info.cores == 0 {
        return changed;
    }

    let c = &info.caches;
    let (data_tip, instruction_tip, l4_tip) = (
        model.text(TIP_L1_DATA),
        model.text(TIP_L1_INSTRUCTION),
        model.text(TIP_L4),
    );
    // L1 is data plus instruction.
    let l1_kib = c.l1d.map_or(0, |l| l.total_kib) + c.l1i.map_or(0, |l| l.total_kib);
    changed |= cache_row(model, ROW_L1, l1_kib, |o| {
        let mut sep = "";
        if let Some(d) = c.l1d {
            data_tip.write(o, &[&Instances(d)])?;
            sep = " + ";
        }
        if let Some(i) = c.l1i {
            o.write_str(sep)?;
            instruction_tip.write(o, &[&Instances(i)])?;
        }
        Ok(())
    });
    changed |= cache_row(model, ROW_L2, c.l2.map_or(0, |l| l.total_kib), |o| {
        c.l2.map_or(Ok(()), |l| write_instances(o, l))
    });
    changed |= cache_row(model, ROW_L3, c.l3.map_or(0, |l| l.total_kib), |o| {
        if let Some(l3) = c.l3 {
            write_with(o, |s| units::count(s, l3.total_kib))?;
            o.write_str(" KiB")?;
            o.write_str(DOT)?;
            write_instances(o, l3)?;
        }
        if let Some(l4) = c.l4 {
            if c.l3.is_some() {
                o.write_str(DOT)?;
            }
            let size = Formatted(|s: &mut String| units::cache_size(s, l4.total_kib));
            l4_tip.write(o, &[&size])?;
        }
        Ok(())
    });

    changed |= update_static_p2(model, info);
    changed
}

/// Rows 15 to 18 and 22 to 26, and whether row 10 is shown.
fn update_static_p2(model: &mut KeyValueModel, info: &CpuStaticInfo) -> bool {
    let mut changed = false;
    // No zone hides the power row; a zone shows it and update_live fills it.
    let power_hidden = row(model, ROW_POWER).hidden;
    match info.rapl {
        Some(RaplState::NoZone) if !power_hidden => {
            model.hide(ROW_POWER);
            changed = true;
        }
        Some(RaplState::Unreadable | RaplState::Readable) if power_hidden => {
            model.show(ROW_POWER);
            changed = true;
        }
        _ => {}
    }

    let v = info.virtualization;
    changed |= if v.validity == Validity::Valid {
        let word = model.text(match v.value {
            VirtState::EnabledKvm => VIRT_ENABLED,
            VirtState::SupportedNoKvm => VIRT_NO_KVM,
            VirtState::NotSupported => VIRT_NOT_SUPPORTED,
        });
        model.set_text(ROW_VIRTUALIZATION, word.text())
    } else {
        static_fallback(model, ROW_VIRTUALIZATION, v.validity)
    };

    // systemd reports "" on bare metal.
    let vm = &info.running_in_vm;
    changed |= match (vm.validity, vm.value.is_empty()) {
        (Validity::Valid, true) => {
            let no = model.text(VM_NO);
            model.set_text(ROW_RUNNING_IN_VM, no.text())
        }
        (Validity::Valid, false) => model.set_text(ROW_RUNNING_IN_VM, &vm.value),
        (other, _) => static_fallback(model, ROW_RUNNING_IN_VM, other),
    };

    changed |= update_security(model, info);

    changed |= match info.hypervisor.as_deref() {
        Some(vendor) => model.set_text(ROW_HYPERVISOR, vendor),
        None => {
            let none = model.text(FEATURE_NONE);
            model.set_text(ROW_HYPERVISOR, none.text())
        }
    };

    changed |= static_text(model, ROW_CPUFREQ_DRIVER, &info.driver);
    changed |= static_text(model, ROW_GOVERNOR, &info.governor);
    changed |= static_text(model, ROW_EPP, &info.epp);
    changed |= static_text(model, ROW_PLATFORM_PROFILE, &info.platform_profile);
    changed |= static_text(model, ROW_MICROCODE, &info.microcode);
    changed
}

/// Row 17: the active features in PERF-037 order, or "None". The tooltip has the vulnerability
/// counts and names each source that could not be read.
fn update_security(model: &mut KeyValueModel, info: &CpuStaticInfo) -> bool {
    let mut changed = with_text(|o| {
        // Both lockdown modes are one feature, and the worker pushes at most one of them.
        let mut first = true;
        for f in &info.security {
            if !first {
                o.push_str(LIST_SEP);
            }
            first = false;
            o.push_str(model.text(feature_key(*f)).text());
        }
        if first {
            o.push_str(model.text(FEATURE_NONE).text());
        }
        model.set_text(ROW_SECURITY, o)
    });

    if info.vulns.is_none() && info.security_unreadable.is_empty() {
        return changed | clear_tip(model, ROW_SECURITY);
    }
    let vulns_tip = model.text(TIP_VULNS);
    let could_not_read = model.words().could_not_read.clone();
    changed |= set_tip(model, ROW_SECURITY, |o| {
        let mut sep = "";
        if let Some(v) = info.vulns {
            vulns_tip.write(o, &[&v.mitigated, &v.vulnerable, &v.not_affected])?;
            sep = DOT;
        }
        for source in &info.security_unreadable {
            o.write_str(sep)?;
            could_not_read.write(o, &[source])?;
            sep = DOT;
        }
        Ok(())
    });
    changed
}

fn feature_key(f: SecurityFeature) -> &'static str {
    match f {
        SecurityFeature::SecureBoot => FEATURE_SECURE_BOOT,
        SecurityFeature::LockdownIntegrity | SecurityFeature::LockdownConfidentiality => {
            FEATURE_LOCKDOWN
        }
        SecurityFeature::MemoryEncryption => FEATURE_MEMORY_ENCRYPTION,
        SecurityFeature::UserShadowStack => FEATURE_SHADOW_STACK,
        SecurityFeature::Iommu => FEATURE_IOMMU,
    }
}

/// A static text row: the text when Valid, else its fallback word.
fn static_text(model: &mut KeyValueModel, label: &'static str, f: &InvField<Arc<str>>) -> bool {
    if f.validity == Validity::Valid {
        model.set_text(label, &f.value)
    } else {
        static_fallback(model, label, f.validity)
    }
}

/// The word for a static field that is not Valid. Missing reads as collecting: the inventory
/// has not filled the field yet.
fn static_fallback(model: &mut KeyValueModel, label: &'static str, v: Validity) -> bool {
    match v {
        Validity::Unsupported => model.set_fallback(label, v, Fallback::NotSupported, None),
        Validity::Missing => {
            model.set_fallback(label, Validity::WarmingUp, Fallback::Unavailable, None)
        }
        _ => model.set_fallback(label, v, Fallback::Unavailable, None),
    }
}

/// Builds a text in a reused per-thread buffer and passes it to `f`, so an unchanged text costs
/// no allocation.
fn with_text<R>(f: impl FnOnce(&mut String) -> R) -> R {
    thread_local! {
        static TEXT: RefCell<String> = const { RefCell::new(String::new()) };
    }
    TEXT.with_borrow_mut(|s| {
        s.clear();
        f(s)
    })
}

/// The validity and Stale age shared by the live rows.
struct Live<'a> {
    age_ns: u64,
    error: Option<&'a str>,
}

impl Live<'_> {
    /// Sets one row from a metric: Valid formats on a bits change, Stale keeps the last value
    /// (formatting it only if the row has none) and marks it, the rest show a fallback word.
    fn row<T: Copy + Default>(
        &self,
        model: &mut KeyValueModel,
        label: &'static str,
        m: Metric<T>,
        bits: impl Fn(T) -> u64,
        write: impl FnOnce(&mut String, T),
    ) -> bool {
        match m.validity {
            Validity::Valid => model.set_valid(label, bits(m.value), |o| write(o, m.value)),
            Validity::Stale => {
                let b = bits(m.value);
                let r = row(model, label);
                let mut changed = false;
                if !(shows_value(r.validity) && r.bits == b) {
                    changed |= model.set_valid(label, b, |o| write(o, m.value));
                }
                changed | model.set_stale(label, self.age_ns)
            }
            v => self.fallback(model, label, v),
        }
    }

    fn fallback(&self, model: &mut KeyValueModel, label: &'static str, v: Validity) -> bool {
        match v {
            Validity::Unsupported => model.set_fallback(label, v, Fallback::NotSupported, None),
            // The CPU never goes away; a Missing CPU value reads as Unavailable.
            Validity::Missing => {
                model.set_fallback(label, Validity::Unsupported, Fallback::Unavailable, None)
            }
            _ => model.set_fallback(label, v, Fallback::Unavailable, self.error),
        }
    }
}

/// Row 10: RAPL package watts with Package and Core in the tooltip. Hidden while
/// [`update_static`] has not seen a RAPL zone. A zone the user cannot read (Restricted, or
/// Unsupported) reads "Needs setup", a link to Additional Setup (PERF-013, PERF-038).
fn update_power(model: &mut KeyValueModel, sys: &SystemSnapshot, live: &Live) -> bool {
    if row(model, ROW_POWER).hidden {
        return false;
    }
    let (pkg, core) = (sys.cpu.package_power_w, sys.cpu.core_power_w);
    if matches!(pkg.validity, Validity::Restricted | Validity::Unsupported) {
        return model.set_fallback(ROW_POWER, Validity::Unsupported, Fallback::NeedsSetup, None);
    }
    let mut changed = live.row(model, ROW_POWER, pkg, f32_bits, units::watts);
    if shows_value(pkg.validity) {
        changed |= if shows_value(core.validity) {
            let tip = model.text(TIP_POWER);
            set_tip(model, ROW_POWER, |o| {
                tip.write(o, &[&Watts(pkg.value), &Watts(core.value)])
            })
        } else {
            clear_tip(model, ROW_POWER)
        };
    }
    changed
}

/// Watts with one decimal and no unit, for the Power draw tooltip.
struct Watts(f32);

impl fmt::Display for Watts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_with(f, |s| {
            units::watts(s, self.0);
            if s.ends_with(" W") {
                s.truncate(s.len() - 2);
            }
        })
    }
}

/// Row 11: hidden when there is no CPU hwmon chip (PERF-013), else the main reading with every
/// sensor of the chip in the tooltip.
fn update_temperature(
    model: &mut KeyValueModel,
    sys: &SystemSnapshot,
    live: &Live,
    sensors: &[CpuSensor],
) -> bool {
    let temp = sys.cpu.temp_c;
    if matches!(temp.validity, Validity::Unsupported | Validity::Missing) {
        if row(model, ROW_TEMPERATURE).hidden {
            return false;
        }
        model.hide(ROW_TEMPERATURE);
        return true;
    }
    let mut changed = false;
    if row(model, ROW_TEMPERATURE).hidden {
        model.show(ROW_TEMPERATURE);
        changed = true;
    }
    changed |= live.row(model, ROW_TEMPERATURE, temp, f32_bits, units::celsius);
    if !shows_value(temp.validity) {
        return changed;
    }
    let readings = || {
        sensors.iter().filter_map(|s| {
            let i = sys.sensors.binary_search_by_key(&s.idx, |(i, _)| *i).ok()?;
            let value = sys.sensors[i].1.value;
            shows_value(value.validity).then_some((&*s.label, value.value))
        })
    };
    changed |= if readings().next().is_none() {
        clear_tip(model, ROW_TEMPERATURE)
    } else {
        set_tip(model, ROW_TEMPERATURE, |o| {
            for (n, (label, c)) in readings().enumerate() {
                if n > 0 {
                    o.write_str(DOT)?;
                }
                o.write_str(label)?;
                o.write_char(' ')?;
                write_with(o, |s| units::celsius(s, c))?;
            }
            Ok(())
        })
    };
    changed
}

/// A cache row: the total in the cache format with `tip`, or "Not reported" when the level has
/// no cache.
fn cache_row(
    model: &mut KeyValueModel,
    label: &'static str,
    kib: u64,
    tip: impl Fn(&mut dyn fmt::Write) -> fmt::Result,
) -> bool {
    if kib == 0 {
        return model.set_fallback(label, Validity::Unsupported, Fallback::NotReported, None);
    }
    model.set_valid(label, kib, |o| units::cache_size(o, kib)) | set_tip(model, label, tip)
}

/// "16 × 32 KiB".
fn write_instances(o: &mut dyn fmt::Write, l: CacheLevelInfo) -> fmt::Result {
    write!(o, "{} × ", l.instances)?;
    write_with(o, |s| units::cache_size(s, l.instance_kib))
}

/// [`write_instances`] as a template argument.
struct Instances(CacheLevelInfo);

impl fmt::Display for Instances {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_instances(f, self.0)
    }
}

/// A `units` formatter as a template argument.
struct Formatted<F>(F);

impl<F: Fn(&mut String)> fmt::Display for Formatted<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_with(f, &self.0)
    }
}

/// Runs a `units` formatter into `o` through a reused per-thread buffer, so the compare path in
/// [`set_tip`] does not allocate.
fn write_with(o: &mut dyn fmt::Write, f: impl FnOnce(&mut String)) -> fmt::Result {
    thread_local! {
        static SCRATCH: RefCell<String> = const { RefCell::new(String::new()) };
    }
    SCRATCH.with_borrow_mut(|s| {
        s.clear();
        f(s);
        o.write_str(s)
    })
}

/// The row with `label`; it exists in every model from [`cpu_details_model`].
fn row<'a>(model: &'a KeyValueModel, label: &str) -> &'a super::details::KvRow {
    model
        .rows
        .iter()
        .find(|r| r.label == label)
        .expect("row from cpu_details_model")
}

/// Valid and Stale show a formatted value; the other states show a word.
fn shows_value(v: Validity) -> bool {
    matches!(v, Validity::Valid | Validity::Stale)
}

fn f32_bits(v: f32) -> u64 {
    u64::from(v.to_bits())
}

/// Bits per load average: hundredths fit in 21 bits up to a load of 20 971.
const LOAD_BITS: u32 = 21;
const LOAD_MAX: u64 = (1 << LOAD_BITS) - 1;

/// The three load averages in hundredths, rounded, packed 21 bits each. Negative and NaN give 0.
fn load_bits(v: [f32; 3]) -> u64 {
    v.iter().fold(0, |acc, &x| {
        let h = (f64::from(x) * 100.0).round();
        // Saturating float to int cast: NaN gives 0, the max is clamped below.
        let h = (h as u64).min(LOAD_MAX);
        acc << LOAD_BITS | h
    })
}

/// "1.24 0.98 0.87" from [`load_bits`].
fn write_load(o: &mut String, bits: u64) {
    for i in 0..3 {
        let h = bits >> (LOAD_BITS * (2 - i)) & LOAD_MAX;
        if i > 0 {
            o.push(' ');
        }
        let _ = write!(o, "{}.{:02}", h / 100, h % 100);
    }
}

/// A Unix time shown by [`write_local`], with the word for a time that does not convert.
struct LocalTime<'a>(i64, &'a str);

impl fmt::Display for LocalTime<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_with(f, |s| write_local(s, self.0, self.1))
    }
}

/// "YYYY-MM-DD HH:MM:SS" in the system time zone, or `unknown`.
fn write_local(out: &mut String, unix_s: i64, unknown: &str) {
    match jiff::Timestamp::from_second(unix_s) {
        Ok(ts) => {
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
        }
        Err(_) => out.push_str(unknown),
    }
}

/// Sets the row's tooltip to the text `write` produces. The text is first compared with the
/// current tooltip in place, so an unchanged tooltip costs no allocation. True when it changed.
fn set_tip(
    model: &mut KeyValueModel,
    label: &'static str,
    write: impl Fn(&mut dyn fmt::Write) -> fmt::Result,
) -> bool {
    if let Some(current) = row(model, label).tooltip.as_deref() {
        let mut cmp = EqWriter {
            rest: current,
            equal: true,
        };
        if write(&mut cmp).is_ok() && cmp.equal && cmp.rest.is_empty() {
            return false;
        }
    }
    let mut text = String::new();
    let _ = write(&mut text);
    model.set_tooltip(label, Some(SharedString::from(text)));
    true
}

fn clear_tip(model: &mut KeyValueModel, label: &'static str) -> bool {
    if row(model, label).tooltip.is_none() {
        return false;
    }
    model.set_tooltip(label, None);
    true
}

/// A `fmt::Write` that checks the written text against `rest` without storing it.
struct EqWriter<'a> {
    rest: &'a str,
    equal: bool,
}

impl fmt::Write for EqWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        match self.rest.strip_prefix(s) {
            Some(rest) if self.equal => self.rest = rest,
            _ => self.equal = false,
        }
        Ok(())
    }
}
