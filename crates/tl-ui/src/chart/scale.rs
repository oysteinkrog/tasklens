//! Y scale rules and axis rounding (UI-PRF-006, ADR-009 rule 9, PERF-018, PERF-019, PERF-030,
//! canonical table (d)).
//!
//! `compute` runs on every chart prepaint and does not allocate: the text is written into the
//! caller's reused `String`, and `SharedString` stores texts up to 23 bytes inline (`SmolStr`).
//! Every axis text is shorter than that.

use core::fmt::Write;

use gpui_kit::SharedString;
use tl_model::settings::NetworkUnit;
use tl_model::units::{ceil_125, ceil_pow2};

use super::{ValueFormat, YScale};

/// How long the visible max must stay below half the span before the scale falls.
pub const FALL_AFTER_NS: u64 = 10_000_000_000;

/// Lowest `AutoRate` ceiling: 1 KiB/s for IEC rates, 1 Kbps for bit rates.
const MIN_RATE_IEC: f32 = 1024.0;
const MIN_RATE_BITS: f32 = 1000.0;
/// Rate samples are bytes per second; bit scales and link speeds count bits.
const BITS_PER_BYTE: f32 = 8.0;

/// `StickyTemp` start range in °C.
const TEMP_START_MIN: f32 = 35.0;
const TEMP_START_MAX: f32 = 45.0;

/// Per-chart scale memory, kept across rebuilds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaleState {
    /// Current max (`AutoRate`, `StickyUp`, `StickyTemp` max).
    pub ceiling: f32,
    /// `StickyTemp` min; 0 otherwise.
    pub floor: f32,
    /// Boot ns when the visible max first stayed below half the span.
    pub below_half_since: Option<u64>,
    pub initialised: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleResult {
    pub y_min: f32,
    pub y_max: f32,
    pub axis_text: Option<SharedString>,
}

/// `ceil(x / 5) × 5`.
pub fn ceil5(x: f32) -> f32 {
    (x / 5.0).ceil() * 5.0
}

/// `floor(x / 5) × 5`.
pub fn floor5(x: f32) -> f32 {
    (x / 5.0).floor() * 5.0
}

fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// Maps the visible range to `y_min..y_max` and writes the axis max text (UI-PRF-006).
/// `visible_*` are over the valid samples in the window; a window with no samples passes 0 for
/// both. Non-finite inputs count as 0.
#[allow(
    clippy::too_many_arguments,
    reason = "signature fixed by the bead (E19-004)"
)]
pub fn compute(
    scale: &YScale,
    visible_min: f32,
    visible_max: f32,
    now_boot_ns: u64,
    state: &mut ScaleState,
    fmt: ValueFormat,
    network_unit: NetworkUnit,
    out_text: &mut String,
) -> ScaleResult {
    let vmin = finite_or_zero(visible_min);
    let vmax = finite_or_zero(visible_max);
    out_text.clear();

    let (y_min, y_max) = match *scale {
        YScale::Fixed { min, max } => (min, max),
        YScale::FixedTotal(total) => {
            if total.is_finite() && total > 0.0 {
                (0.0, total)
            } else {
                (0.0, 1.0)
            }
        }
        YScale::Commit(limit) => {
            if !(limit.is_finite() && limit > 0.0) {
                (0.0, vmax.max(1.0))
            } else {
                let y_max = limit.max(vmax);
                if vmax > limit {
                    let _ = write!(out_text, "{:.0}%", (y_max / limit * 100.0).round());
                }
                (0.0, y_max)
            }
        }
        // Samples are bytes per second. The bit scale rounds to 1-2-5 steps in bits and keeps
        // its ceiling in bytes.
        YScale::AutoRate { iec } => {
            let (target, min) = if iec {
                // Rates above u64::MAX bytes/s do not occur; the cast saturates.
                let pow2 = ceil_pow2(f64::from(vmax).ceil() as u64) as f32;
                (pow2, MIN_RATE_IEC)
            } else {
                let bits = ceil_125(f64::from(vmax * BITS_PER_BYTE)) as f32;
                (bits / BITS_PER_BYTE, MIN_RATE_BITS / BITS_PER_BYTE)
            };
            let target = target.max(min);
            let ceiling = sticky_fall(state, target, vmax < state.ceiling / 2.0, now_boot_ns);
            let shown = if iec {
                ceiling
            } else {
                ceiling * BITS_PER_BYTE
            };
            write_rate(out_text, f64::from(shown), iec);
            (0.0, ceiling)
        }
        YScale::LinkSpeed(bps) => {
            let bps = if bps.is_finite() && bps > 0.0 {
                bps
            } else {
                1.0
            };
            match network_unit {
                NetworkUnit::Bits => write_rate(out_text, f64::from(bps), false),
                NetworkUnit::Bytes => {
                    write_rate(out_text, f64::from(bps / BITS_PER_BYTE), true);
                }
            }
            // The link speed counts bits; samples are bytes per second.
            (0.0, bps / BITS_PER_BYTE)
        }
        YScale::StickyTemp => {
            if !state.initialised {
                state.floor = TEMP_START_MIN;
                state.ceiling = TEMP_START_MAX;
                state.below_half_since = None;
                state.initialised = true;
            }
            // Both 0 means no samples in the window: keep the range.
            if vmin != 0.0 || vmax != 0.0 {
                let target = ceil5(vmax).max(TEMP_START_MAX);
                let half = state.floor + (state.ceiling - state.floor) / 2.0;
                state.ceiling = sticky_fall(state, target, vmax < half, now_boot_ns);
                state.floor = state.floor.min(floor5(vmin)).min(TEMP_START_MIN);
            }
            write_number(out_text, state.ceiling);
            out_text.push_str(" °C");
            (state.floor, state.ceiling)
        }
        YScale::StickyUp { hw_max: Some(m) } => {
            let m = if m.is_finite() && m > 0.0 { m } else { 1.0 };
            write_value(out_text, m, fmt, network_unit);
            (0.0, m)
        }
        YScale::StickyUp { hw_max: None } => {
            let target = ceil_125(f64::from(vmax)) as f32;
            if !state.initialised || target > state.ceiling {
                state.ceiling = target;
                state.initialised = true;
            }
            write_value(out_text, state.ceiling, fmt, network_unit);
            (0.0, state.ceiling)
        }
        YScale::Stacked(n) => {
            let top = 100.0 * f32::from(n.max(1));
            let _ = write!(out_text, "{top:.0}%");
            (0.0, top)
        }
        YScale::Symmetric => {
            let m = (ceil_125(f64::from(vmin.abs().max(vmax.abs()))) as f32).max(1.0);
            out_text.push('±');
            write_number(out_text, m);
            out_text.push_str(" W");
            (-m, m)
        }
    };

    let axis_text = (!out_text.is_empty()).then(|| SharedString::new(out_text.as_str()));
    ScaleResult {
        y_min,
        y_max,
        axis_text,
    }
}

/// Shared rise and fall rule for `AutoRate` and `StickyTemp`: rises to `target` at once; when
/// `below_half` has held for `FALL_AFTER_NS` the ceiling falls to `target`. Returns the ceiling.
fn sticky_fall(state: &mut ScaleState, target: f32, below_half: bool, now: u64) -> f32 {
    if !state.initialised || target > state.ceiling {
        state.ceiling = target;
        state.below_half_since = None;
        state.initialised = true;
    } else if below_half {
        let since = *state.below_half_since.get_or_insert(now);
        if now.saturating_sub(since) >= FALL_AFTER_NS {
            state.ceiling = target;
            state.below_half_since = None;
        }
    } else {
        state.below_half_since = None;
    }
    state.ceiling
}

/// One decimal, with ".0" dropped: "5", "2.5", "1600".
fn write_number(out: &mut String, v: f32) {
    let start = out.len();
    let _ = write!(out, "{:.1}", v + 0.0);
    if out.ends_with(".0") {
        out.truncate(out.len() - 2);
    }
    if &out[start..] == "-0" {
        out.truncate(start);
        out.push('0');
    }
}

/// Axis rate: IEC bytes per second ("4 MiB/s") or SI bits per second ("100 Mbps"), stepping up
/// while the value is at least one unit of the next size.
fn write_rate(out: &mut String, v: f64, iec: bool) {
    const IEC: [&str; 5] = [" B/s", " KiB/s", " MiB/s", " GiB/s", " TiB/s"];
    const SI: [&str; 5] = [" bps", " Kbps", " Mbps", " Gbps", " Tbps"];
    let (base, units) = if iec { (1024.0, &IEC) } else { (1000.0, &SI) };
    write_scaled(out, v, base, units);
}

fn write_scaled(out: &mut String, mut v: f64, base: f64, units: &[&str]) {
    let mut i = 0;
    while v >= base && i + 1 < units.len() {
        v /= base;
        i += 1;
    }
    // Axis values are already rounded; f32 keeps one decimal exact enough.
    write_number(out, v as f32);
    out.push_str(units[i]);
}

/// Axis max for `StickyUp`, by the chart's value format.
fn write_value(out: &mut String, v: f32, fmt: ValueFormat, network_unit: NetworkUnit) {
    match fmt {
        ValueFormat::Watts => {
            write_number(out, v);
            out.push_str(" W");
        }
        ValueFormat::Mhz => {
            write_number(out, v);
            out.push_str(" MHz");
        }
        ValueFormat::Rpm => {
            write_number(out, v);
            out.push_str(" RPM");
        }
        ValueFormat::Celsius => {
            write_number(out, v);
            out.push_str(" °C");
        }
        ValueFormat::Percent => {
            write_number(out, v);
            out.push('%');
        }
        ValueFormat::PercentOf { total } => {
            write_number(out, super::labels::percent_of(v, total));
            out.push('%');
        }
        ValueFormat::Bytes => {
            const IEC: [&str; 5] = [" B", " KiB", " MiB", " GiB", " TiB"];
            write_scaled(out, f64::from(v), 1024.0, &IEC);
        }
        ValueFormat::RateIec => write_rate(out, f64::from(v), true),
        ValueFormat::RateNetwork => {
            write_rate(out, f64::from(v), network_unit == NetworkUnit::Bytes);
        }
        ValueFormat::Count => write_number(out, v),
    }
}
