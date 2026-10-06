//! The CPU page's Details grid, P1 rows (docs/ui/performance.md UI-PRF-024; PERF-033 to
//! PERF-036, PERF-039, PERF-040).
//!
//! Rows 1 to 12 are live and update at 1 Hz from `SystemSnapshot`; rows 13, 14 and 19 to 21 are
//! static and update from `CpuStaticInfo`. Each value is formatted only when its bits change, and
//! tooltips are compared in place before they are rebuilt, so a tick where nothing changed does
//! not allocate.
//!
//! The tooltip texts match `perf-cpu-tip-*` in `en-US.ftl`. They are English constants until the
//! model gets a strings context, like the fallback words in `details.rs`.

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use gpui_kit::SharedString;
use tl_model::devices::{DeviceCatalog, DeviceIdentity};
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{CacheLevelInfo, CpuStaticInfo, SystemSnapshot};
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
pub const ROW_TEMPERATURE: &str = "perf-cpu-row-temperature";
pub const ROW_LOAD_AVERAGE: &str = "perf-cpu-row-load-average";
pub const ROW_CORES: &str = "perf-cpu-row-cores";
pub const ROW_LOGICAL: &str = "perf-cpu-row-logical-processors";
pub const ROW_L1: &str = "perf-cpu-row-l1";
pub const ROW_L2: &str = "perf-cpu-row-l2";
pub const ROW_L3: &str = "perf-cpu-row-l3";

/// The P1 rows in grid order (rows 10, 15 to 18 and 22 to 26 are P2).
pub const P1_ROWS: [&str; 16] = [
    ROW_UTILIZATION,
    ROW_KERNEL_TIME,
    ROW_SPEED,
    ROW_PROCESSES,
    ROW_THREADS,
    ROW_HANDLES,
    ROW_UP_TIME,
    ROW_BASE_SPEED,
    ROW_SOCKETS,
    ROW_TEMPERATURE,
    ROW_LOAD_AVERAGE,
    ROW_CORES,
    ROW_LOGICAL,
    ROW_L1,
    ROW_L2,
    ROW_L3,
];

/// Followed by the fastest core's MHz.
const TIP_FASTEST: &str = "Fastest core: ";
const TIP_BOGOMIPS: &str = "Reported by the kernel at boot; the current clock is not available";
const TIP_HANDLES: &str = "Open file descriptors, system-wide";
/// Followed by the boot time.
const TIP_BOOT: &str = "Boot time ";
const TIP_LOADAVG: &str = "1, 5 and 15 minutes";
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
    /// The clock comes from the BogoMIPS lines of /proc/cpuinfo.
    pub bogomips: bool,
    /// `SourceHealth.last_error` of the CPU source, shown for Failed values.
    pub cpu_error: Option<&'a str>,
    /// From [`cpu_temp_sensors`].
    pub temp_sensors: &'a [CpuSensor],
}

/// A Details model with the 16 P1 rows, collecting.
pub fn cpu_details_model() -> KeyValueModel {
    KeyValueModel::new(&P1_ROWS)
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

/// Updates rows 1 to 9, 11 and 12. True when any shown text or state changed.
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

    let bogomips = inp.bogomips;
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
            set_tip(model, ROW_SPEED, |o| o.write_str(TIP_BOGOMIPS))
        } else if shows_value(fastest.validity) {
            set_tip(model, ROW_SPEED, |o| {
                o.write_str(TIP_FASTEST)?;
                write!(o, "{} MHz", fastest.value)
            })
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
        changed |= set_tip(model, ROW_HANDLES, |o| o.write_str(TIP_HANDLES));
    }

    // CLOCK_BOOTTIME is the up time, so this row is always Valid.
    let up_s = inp.now_boot_ns / 1_000_000_000;
    changed |= model.set_valid(ROW_UP_TIME, up_s, |o| units::uptime(o, up_s));
    // btime does not change within a boot, so the tooltip is formatted once.
    if sys.boot_time_s > 0 && row(model, ROW_UP_TIME).tooltip.is_none() {
        let mut text = String::from(TIP_BOOT);
        write_local(&mut text, sys.boot_time_s);
        model.set_tooltip(ROW_UP_TIME, Some(SharedString::from(text)));
        changed = true;
    }

    changed |= live.row(model, ROW_BASE_SPEED, cpu.base_mhz, u64::from, |o, v| {
        units::mhz(o, v as f32)
    });
    changed |= live.row(model, ROW_SOCKETS, cpu.sockets, u64::from, |o, v| {
        units::count(o, u64::from(v))
    });

    changed |= update_temperature(model, sys, &live, inp.temp_sensors);

    changed |= live.row(model, ROW_LOAD_AVERAGE, cpu.load_avg, load_bits, |o, _| {
        // Formatted from the bits, so the text and the bits always agree.
        write_load(o, load_bits(cpu.load_avg.value));
    });
    if shows_value(cpu.load_avg.validity) {
        changed |= set_tip(model, ROW_LOAD_AVERAGE, |o| o.write_str(TIP_LOADAVG));
    }
    changed
}

/// Updates rows 13, 14 and 19 to 21. True when any shown text or state changed.
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
    // L1 is data plus instruction.
    let l1_kib = c.l1d.map_or(0, |l| l.total_kib) + c.l1i.map_or(0, |l| l.total_kib);
    changed |= cache_row(model, ROW_L1, l1_kib, |o| {
        let mut sep = "";
        if let Some(d) = c.l1d {
            write_instances(o, d)?;
            o.write_str(" data")?;
            sep = " + ";
        }
        if let Some(i) = c.l1i {
            o.write_str(sep)?;
            write_instances(o, i)?;
            o.write_str(" instruction")?;
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
            o.write_str("L4 ")?;
            write_with(o, |s| units::cache_size(s, l4.total_kib))?;
        }
        Ok(())
    });
    changed
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

/// "YYYY-MM-DD HH:MM:SS" in the system time zone.
fn write_local(out: &mut String, unix_s: i64) {
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
        Err(_) => out.push_str("Unknown"),
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
