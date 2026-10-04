//! Number and unit formatting, canonical table (d).
//!
//! Every formatter appends to a caller-provided `String` and never clears it. Nothing here
//! allocates when `out` has room (ADR-003). The decimal point is always "." (R-081).
//!
//! Measured zero (PROC-003): a value that is 0, or that rounds to 0 at the printed precision,
//! prints the zero form of its row ("0%", "0 B", "0 MiB/s"). Negative and NaN input print the
//! zero form too. Callers format only `Valid` and `Stale` values; other states use glyphs.

use core::fmt::Write;

use serde::{Deserialize, Serialize};

const KIB: f64 = 1024.0;
const MIB: f64 = 1024.0 * 1024.0;

/// Writes `v` with `prec` decimals and `suffix`. If `v` is not positive, or the printed digits
/// are all zero, the written text is replaced by `zero`.
fn write_or_zero(out: &mut String, v: f64, prec: usize, suffix: &str, zero: &str) {
    // `v <= 0.0` also catches -0.0.
    if v.is_nan() || v <= 0.0 {
        out.push_str(zero);
        return;
    }
    let start = out.len();
    let _ = write!(out, "{v:.prec$}");
    if out.as_bytes()[start..]
        .iter()
        .all(|&b| b == b'0' || b == b'.')
    {
        out.truncate(start);
        out.push_str(zero);
        return;
    }
    out.push_str(suffix);
}

/// Writes `v` (already in `units[0]`) with one decimal, stepping to the next unit while the value
/// rounded to one decimal is >= `base`. `base` is 1024 for IEC and 1000 for SI.
fn write_scaled_1dp(out: &mut String, mut v: f64, base: f64, units: &[&str], zero: &str) {
    // {:.1} prints base.0 or more exactly when v >= base - 0.05.
    let step = base - 0.05;
    let mut i = 0;
    while v >= step && i + 1 < units.len() {
        v /= base;
        i += 1;
    }
    write_or_zero(out, v, 1, units[i], zero);
}

/// "11.9%", "3.4%", "100.0%"; zero form "0%". One decimal, {:.1}.
pub fn percent(out: &mut String, pct: f32) {
    write_or_zero(out, f64::from(pct), 1, "%", "0%");
}

/// Copy details CPU only (R-094): "3.27%", two decimals; zero form "0%".
pub fn percent_2dp(out: &mut String, pct: f32) {
    write_or_zero(out, f64::from(pct), 2, "%", "0%");
}

/// IEC sizes, base 1024, units B KiB MiB GiB TiB. Below 1024: integer bytes "512 B";
/// zero "0 B". From 1 KiB: two decimals, "16.00 KiB", "1.22 GiB". The unit is chosen after
/// rounding to two decimals: if the rounded value is >= 1024.00 the next unit is used.
/// TiB is the largest unit (u64::MAX prints "16777216.00 TiB").
pub fn bytes(out: &mut String, n: u64) {
    const UNITS: [&str; 4] = [" KiB", " MiB", " GiB", " TiB"];
    if n < 1024 {
        let _ = write!(out, "{n} B");
        return;
    }
    // Precision loss in the cast is far below the two printed decimals.
    let mut v = n as f64 / KIB;
    // {:.2} prints 1024.00 or more exactly when v >= 1023.995.
    let mut i = 0;
    while v >= KIB - 0.005 && i + 1 < UNITS.len() {
        v /= KIB;
        i += 1;
    }
    let _ = write!(out, "{v:.2}{}", UNITS[i]);
}

/// "34.68 GiB / 63.16 GiB": bytes(used), " / ", bytes(total).
pub fn bytes_of(out: &mut String, used: u64, total: u64) {
    bytes(out, used);
    out.push_str(" / ");
    bytes(out, total);
}

/// Whole MiB for copy details lines: round to nearest MiB, "812 MiB"; zero form "0 MiB".
pub fn mib_whole(out: &mut String, n: u64) {
    const MIB_U: u64 = 1024 * 1024;
    let whole = n / MIB_U + u64::from(n % MIB_U >= MIB_U / 2);
    let _ = write!(out, "{whole} MiB");
}

/// Disk and I/O rates in tables: fixed MiB/s, one decimal, "0.4 MiB/s"; zero form "0 MiB/s".
pub fn rate_mib_fixed(out: &mut String, bytes_per_s: f64) {
    write_or_zero(out, bytes_per_s / MIB, 1, " MiB/s", "0 MiB/s");
}

/// Disk rates on Performance graphs and stats: auto IEC unit KiB/s, MiB/s, GiB/s, one decimal,
/// "12.3 MiB/s". Below 1 KiB/s stays in KiB/s ("0.5 KiB/s"). The unit steps up when the value
/// rounded to one decimal is >= 1024.0. Zero form "0 KiB/s".
pub fn rate_iec_auto(out: &mut String, bytes_per_s: f64) {
    rate_iec_auto_with_zero(out, bytes_per_s, "0 KiB/s");
}

fn rate_iec_auto_with_zero(out: &mut String, bytes_per_s: f64, zero: &str) {
    const UNITS: [&str; 3] = [" KiB/s", " MiB/s", " GiB/s"];
    write_scaled_1dp(out, bytes_per_s / KIB, KIB, &UNITS, zero);
}

/// Network rate unit, settings `units.network` = "bits" | "bytes".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetUnit {
    #[default]
    Bits,
    Bytes,
}

/// Network rates (R-015, PERF-025). Input is bytes per second.
/// Bits (SI, base 1000, bits = bytes × 8):
///   auto_scale false: fixed Mbps, one decimal, "2.3 Mbps"; zero form "0 Mbps".
///   auto_scale true: Kbps below 1 000 000 bit/s (including values under 1 Kbps, "0.4 Kbps"),
///   Mbps below 1 000 000 000, else Gbps; one decimal; step up when the rounded value >= 1000.0;
///   zero form "0 Mbps".
/// Bytes (IEC): auto_scale false: same as rate_mib_fixed ("1.5 MiB/s", zero "0 MiB/s");
///   auto_scale true: same as rate_iec_auto but with zero form "0 MiB/s".
pub fn net_rate(out: &mut String, bytes_per_s: f64, unit: NetUnit, auto_scale: bool) {
    const BIT_UNITS: [&str; 3] = [" Kbps", " Mbps", " Gbps"];
    match (unit, auto_scale) {
        (NetUnit::Bits, false) => {
            write_or_zero(out, bytes_per_s * 8.0 / 1e6, 1, " Mbps", "0 Mbps");
        }
        (NetUnit::Bits, true) => {
            write_scaled_1dp(out, bytes_per_s * 8.0 / 1e3, 1000.0, &BIT_UNITS, "0 Mbps");
        }
        (NetUnit::Bytes, false) => rate_mib_fixed(out, bytes_per_s),
        (NetUnit::Bytes, true) => rate_iec_auto_with_zero(out, bytes_per_s, "0 MiB/s"),
    }
}

/// Smallest power of two >= n; 0 gives 1; values above 1 << 63 saturate at 1 << 63. Disk axis maximum.
pub fn ceil_pow2(n: u64) -> u64 {
    n.checked_next_power_of_two().unwrap_or(1 << 63)
}

/// Smallest value of the form {1, 2, 5} × 10^k that is >= x (k may be negative). Network axis maximum.
/// x <= 0 or NaN gives 1.0.
pub fn ceil_125(x: f64) -> f64 {
    if x.is_nan() || x <= 0.0 {
        return 1.0;
    }
    // log10 can be off by one near a power of ten, so try the decade below as well.
    // The cast is safe: log10 of a positive f64 lies in about -324..=309 (or is +inf, which
    // saturates to i32::MAX and gives +inf below).
    let k = x.log10().floor() as i32 - 1;
    for kk in [k, k.saturating_add(1), k.saturating_add(2)] {
        for m in [1.0, 2.0, 5.0] {
            let c = scale10(m, kk);
            if c >= x {
                return c;
            }
        }
    }
    scale10(1.0, k.saturating_add(3))
}

/// `m × 10^k`. Divides for negative k so that 0.5, 0.2 and 0.1 come out exact.
fn scale10(m: f64, k: i32) -> f64 {
    if k >= 0 {
        return m * 10f64.powi(k);
    }
    let p = 10f64.powi(k.saturating_neg());
    if p.is_finite() {
        m / p
    } else {
        // Subnormal range: 10^-k overflows, multiply by the tiny power instead.
        m * 10f64.powi(k)
    }
}

/// Digit group separator for `count`.
pub const THIN_SPACE: char = '\u{2009}';

/// Digits grouped in threes with U+2009: "0", "999", "1 000", "12 345" (thin spaces).
pub fn count(out: &mut String, n: u64) {
    // u64::MAX has 20 digits.
    let mut digits = [0u8; 20];
    let mut len = 0;
    let mut rest = n;
    loop {
        // `rest % 10` is below 10, so the cast cannot truncate.
        digits[len] = b'0' + (rest % 10) as u8;
        len += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        out.push(char::from(digits[i]));
        if i > 0 && i % 3 == 0 {
            out.push(THIN_SPACE);
        }
    }
}

/// CPU time from whole seconds: "h:mm:ss" ("0:00:00", "12:03:44", "999:59:59"); from
/// 3_600_000 s on (past 999:59:59): "Nd hh:mm" with days = s / 86400, e.g. 3_600_000 -> "41d 16:00".
pub fn cpu_time(out: &mut String, secs: u64) {
    if secs < 3_600_000 {
        let _ = write!(
            out,
            "{}:{:02}:{:02}",
            secs / 3600,
            secs % 3600 / 60,
            secs % 60
        );
    } else {
        let _ = write!(
            out,
            "{}d {:02}:{:02}",
            secs / 86_400,
            secs % 86_400 / 3600,
            secs % 3600 / 60
        );
    }
}

/// Uptime (CPU page, Information): "N day(s), Hh Mm Ss"; hours always shown.
/// 9_495 -> "2h 38m 15s"; 2_295 -> "0h 38m 15s"; 95_895 -> "1 day, 2h 38m 15s";
/// 182_295 -> "2 days, 2h 38m 15s"; 0 -> "0h 0m 0s".
pub fn uptime(out: &mut String, secs: u64) {
    match secs / 86_400 {
        0 => {}
        1 => out.push_str("1 day, "),
        days => {
            let _ = write!(out, "{days} days, ");
        }
    }
    let _ = write!(
        out,
        "{}h {}m {}s",
        secs % 86_400 / 3600,
        secs % 3600 / 60,
        secs % 60
    );
}

/// Short ages for tooltips and the Start time age text: >= 1 day "1d 4h"; >= 1 h "4h 12m";
/// >= 1 min "12m 5s"; else "5s" ("0s" for 0). Two parts at most; lower parts are dropped, not rounded.
pub fn short_age(out: &mut String, secs: u64) {
    let _ = if secs >= 86_400 {
        write!(out, "{}d {}h", secs / 86_400, secs % 86_400 / 3600)
    } else if secs >= 3600 {
        write!(out, "{}h {}m", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        write!(out, "{}m {}s", secs / 60, secs % 60)
    } else {
        write!(out, "{secs}s")
    };
}

/// Broken-down local time supplied by the caller (tl-model does no time-zone work).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Start time column: today "HH:MM:SS", otherwise "YYYY-MM-DD HH:MM" (R-007).
pub fn start_time(out: &mut String, t: LocalDateTime, is_today: bool) {
    let _ = if is_today {
        write!(out, "{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
    } else {
        write!(
            out,
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.minute
        )
    };
}

/// Properties "Started" and Boot time: "YYYY-MM-DD HH:MM:SS".
pub fn date_time(out: &mut String, t: LocalDateTime) {
    let _ = write!(
        out,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    );
}

/// Writes `v` rounded to the nearest integer (half away from zero). -0 prints "0" and NaN
/// prints "0".
fn write_round0(out: &mut String, v: f32) {
    let r = if v.is_nan() { 0.0 } else { v.round() + 0.0 };
    let _ = write!(out, "{r:.0}");
}

/// Integer MHz, rounded to nearest: "4631 MHz".
pub fn mhz(out: &mut String, mhz: f32) {
    write_round0(out, mhz);
    out.push_str(" MHz");
}

/// "1905 / 3105 MHz".
pub fn mhz_pair(out: &mut String, cur: f32, max: f32) {
    write_round0(out, cur);
    out.push_str(" / ");
    mhz(out, max);
}

/// Integer °C rounded to nearest, with a space: "52 °C".
pub fn celsius(out: &mut String, c: f32) {
    write_round0(out, c);
    out.push_str(" °C");
}

/// One decimal W: "31.9 W"; zero form "0 W".
pub fn watts(out: &mut String, w: f32) {
    write_or_zero(out, f64::from(w), 1, " W", "0 W");
}

/// "31.9 / 300 W": used with one decimal, limit as an integer rounded to nearest.
/// Used follows the measured-zero rule ("0 / 300 W").
pub fn watts_pair(out: &mut String, used: f32, limit: f32) {
    write_or_zero(out, f64::from(used), 1, "", "0");
    out.push_str(" / ");
    write_round0(out, limit);
    out.push_str(" W");
}

/// Cache sizes from KiB: up to 1023 KiB integer KiB ("512 KiB"); from 1024 KiB in MiB,
/// integer when exact ("8 MiB", "96 MiB"), else one decimal ("1.5 MiB").
pub fn cache_size(out: &mut String, kib: u64) {
    let _ = if kib < 1024 {
        write!(out, "{kib} KiB")
    } else if kib.is_multiple_of(1024) {
        write!(out, "{} MiB", kib / 1024)
    } else {
        // Precision loss in the cast is far below the one printed decimal.
        write!(out, "{:.1} MiB", kib as f64 / KIB)
    };
}

/// Which end of a connection `socket_addr` formats.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddrSide {
    Local,
    Foreign,
}

/// Socket address forms: IPv4 "a.b.c.d:port"; IPv6 "[addr%scope]:port" ("%scope" only when
/// scope is Some); port 0 prints "*" ("0.0.0.0:*", "[::]:*"). Foreign side with an unspecified
/// address and port 0 prints "*:*" (listeners and unconnected UDP).
pub fn socket_addr(
    out: &mut String,
    ip: std::net::IpAddr,
    port: u16,
    scope: Option<&str>,
    side: AddrSide,
) {
    if side == AddrSide::Foreign && port == 0 && ip.is_unspecified() {
        out.push_str("*:*");
        return;
    }
    // IpAddr's Display gives the RFC 5952 compressed form for IPv6.
    let _ = match ip {
        std::net::IpAddr::V4(v4) => write!(out, "{v4}"),
        std::net::IpAddr::V6(v6) => match scope {
            Some(scope) => write!(out, "[{v6}%{scope}]"),
            None => write!(out, "[{v6}]"),
        },
    };
    if port == 0 {
        out.push_str(":*");
    } else {
        let _ = write!(out, ":{port}");
    }
}

/// Response time: milliseconds with two decimals, "0.42 ms"; zero form "0 ms".
pub fn response_ms(out: &mut String, ms: f64) {
    write_or_zero(out, ms, 2, " ms", "0 ms");
}
