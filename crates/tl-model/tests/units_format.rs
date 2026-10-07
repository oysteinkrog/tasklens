//! Formatting cases for `tl_model::units`, canonical table (d).

use proptest::prelude::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tl_model::units::{
    AddrSide, LocalDateTime, NetUnit, bytes, bytes_of, bytes_si_1dp, cache_size, ceil_125,
    ceil_pow2, celsius, count, cpu_time, date_time, mhz, mhz_pair, mib_whole, net_rate, percent,
    percent_2dp, rate_iec_auto, rate_mib_fixed, response_ms, short_age, socket_addr, start_time,
    uptime, watts, watts_pair,
};

fn fmt(f: impl FnOnce(&mut String)) -> String {
    let mut s = String::new();
    f(&mut s);
    s
}

#[test]
fn percent_forms() {
    let cases = [
        (11.94, "11.9%"),
        (3.4, "3.4%"),
        (100.0, "100.0%"),
        (0.0, "0%"),
        (0.04, "0%"),
        (999.95, "1000.0%"),
    ];
    for (v, want) in cases {
        assert_eq!(fmt(|s| percent(s, v)), want, "percent({v})");
    }
}

#[test]
fn percent_2dp_forms() {
    assert_eq!(fmt(|s| percent_2dp(s, 3.27)), "3.27%");
    assert_eq!(fmt(|s| percent_2dp(s, 0.0)), "0%");
}

#[test]
fn bytes_boundaries() {
    let cases = [
        (0, "0 B"),
        (1, "1 B"),
        (512, "512 B"),
        (1023, "1023 B"),
        (1024, "1.00 KiB"),
        (16384, "16.00 KiB"),
        (1_048_575, "1.00 MiB"),
        (1_048_576, "1.00 MiB"),
        (1_310_000_000, "1.22 GiB"),
        (u64::MAX, "16777216.00 TiB"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| bytes(s, n)), want, "bytes({n})");
    }
}

#[test]
fn bytes_si_1dp_boundaries() {
    let cases = [
        (0, "0 B"),
        (999, "999 B"),
        (1000, "1.0 kB"),
        (1049, "1.0 kB"),
        (999_949, "999.9 kB"),
        (999_950, "1.0 MB"),
        (12_000_000_000_000, "12.0 TB"),
        (1_500_000_000_000_000, "1.5 PB"),
        (u64::MAX, "18446.7 PB"),
    ];
    for (n, want) in cases {
        assert_eq!(bytes_si_1dp(n), want, "bytes_si_1dp({n})");
    }
}

#[test]
fn bytes_of_pair() {
    assert_eq!(
        fmt(|s| bytes_of(s, 37_237_000_000, 67_818_000_000)),
        "34.68 GiB / 63.16 GiB"
    );
}

#[test]
fn mib_whole_forms() {
    assert_eq!(fmt(|s| mib_whole(s, 0)), "0 MiB");
    assert_eq!(fmt(|s| mib_whole(s, 851_443_712)), "812 MiB");
    assert_eq!(fmt(|s| mib_whole(s, 524_287)), "0 MiB");
}

#[test]
fn rate_fixed_forms() {
    assert_eq!(fmt(|s| rate_mib_fixed(s, 0.0)), "0 MiB/s");
    assert_eq!(fmt(|s| rate_mib_fixed(s, 419_430.4)), "0.4 MiB/s");
    assert_eq!(fmt(|s| rate_mib_fixed(s, 1023.0)), "0 MiB/s");
}

#[test]
fn rate_auto_forms() {
    assert_eq!(fmt(|s| rate_iec_auto(s, 0.0)), "0 KiB/s");
    assert_eq!(fmt(|s| rate_iec_auto(s, 512.0)), "0.5 KiB/s");
    assert_eq!(fmt(|s| rate_iec_auto(s, 12_897_484.8)), "12.3 MiB/s");
    assert_eq!(fmt(|s| rate_iec_auto(s, 4_294_967_296.0)), "4.0 GiB/s");
}

#[test]
fn rate_auto_step_up() {
    assert_eq!(fmt(|s| rate_iec_auto(s, 1_048_550.0)), "1.0 MiB/s");
}

#[test]
fn net_bits_fixed() {
    assert_eq!(
        fmt(|s| net_rate(s, 287_500.0, NetUnit::Bits, false)),
        "2.3 Mbps"
    );
    assert_eq!(fmt(|s| net_rate(s, 0.0, NetUnit::Bits, false)), "0 Mbps");
    assert_eq!(fmt(|s| net_rate(s, 6.0, NetUnit::Bits, false)), "0 Mbps");
}

#[test]
fn net_bits_auto() {
    assert_eq!(fmt(|s| net_rate(s, 50.0, NetUnit::Bits, true)), "0.4 Kbps");
    assert_eq!(
        fmt(|s| net_rate(s, 287_500.0, NetUnit::Bits, true)),
        "2.3 Mbps"
    );
    assert_eq!(
        fmt(|s| net_rate(s, 250_000_000.0, NetUnit::Bits, true)),
        "2.0 Gbps"
    );
    assert_eq!(fmt(|s| net_rate(s, 0.0, NetUnit::Bits, true)), "0 Mbps");
}

#[test]
fn net_bytes_forms() {
    assert_eq!(
        fmt(|s| net_rate(s, 1_572_864.0, NetUnit::Bytes, false)),
        "1.5 MiB/s"
    );
    assert_eq!(fmt(|s| net_rate(s, 0.0, NetUnit::Bytes, true)), "0 MiB/s");
}

#[test]
fn nan_and_negative_are_zero_forms() {
    assert_eq!(fmt(|s| percent(s, f32::NAN)), "0%");
    assert_eq!(fmt(|s| rate_mib_fixed(s, -5.0)), "0 MiB/s");
    assert_eq!(fmt(|s| percent(s, -0.0)), "0%");
    assert_eq!(fmt(|s| rate_iec_auto(s, f64::NAN)), "0 KiB/s");
    assert_eq!(fmt(|s| net_rate(s, -1.0, NetUnit::Bits, true)), "0 Mbps");
}

#[test]
fn appends_without_realloc() {
    let mut out = String::with_capacity(64);
    out.push('x');
    let cap = out.capacity();
    bytes(&mut out, 1024);
    assert_eq!(out, "x1.00 KiB");
    assert_eq!(out.capacity(), cap);
    assert_eq!(cap, 64);
}

#[test]
fn ceil_125_examples() {
    assert_eq!(ceil_125(1234.0), 2000.0);
    assert_eq!(ceil_125(60.0), 100.0);
    assert_eq!(ceil_125(0.3), 0.5);
    assert_eq!(ceil_125(1000.0), 1000.0);
    assert_eq!(ceil_125(5.0), 5.0);
}

#[test]
fn ceil_pow2_examples() {
    assert_eq!(ceil_pow2(3 * 1024 * 1024), 4 * 1024 * 1024);
    assert_eq!(ceil_pow2(4096), 4096);
    assert_eq!(ceil_pow2(0), 1);
    assert_eq!(ceil_pow2(u64::MAX), 1 << 63);
}

#[test]
fn ceil_helpers_edge_inputs() {
    assert_eq!(ceil_125(0.0), 1.0);
    assert_eq!(ceil_125(-1.0), 1.0);
    assert_eq!(ceil_125(f64::NAN), 1.0);
    assert!(ceil_125(f64::MIN_POSITIVE / 4.0) >= f64::MIN_POSITIVE / 4.0);
}

#[test]
fn ceil_125_tiny_inputs() {
    let cases = [
        f64::MIN_POSITIVE / 2.0,
        f64::MIN_POSITIVE,
        1e-300,
        5e-324,
        3.954899102388e-311,
    ];
    for x in cases {
        let c = ceil_125(x);
        assert!(c >= x, "ceil_125({x:e}) = {c:e}");
        assert!(c.is_normal(), "ceil_125({x:e}) = {c:e}");
    }
    assert_eq!(ceil_125(f64::MIN_POSITIVE), 5e-308);
    assert_eq!(ceil_125(5e-324), 5e-308);
}

fn check(s: &str) {
    assert!(!s.is_empty());
    assert!(s.is_ascii(), "{s:?}");
}

proptest! {
    #[test]
    fn formatters_never_panic(
        n in any::<u64>(),
        m in any::<u64>(),
        p in proptest::num::f32::ANY,
        r in proptest::num::f64::ANY,
    ) {
        check(&fmt(|s| percent(s, p)));
        check(&fmt(|s| percent_2dp(s, p)));
        check(&fmt(|s| bytes(s, n)));
        check(&fmt(|s| bytes_of(s, n, m)));
        check(&fmt(|s| mib_whole(s, n)));
        check(&fmt(|s| rate_mib_fixed(s, r)));
        check(&fmt(|s| rate_iec_auto(s, r)));
        for unit in [NetUnit::Bits, NetUnit::Bytes] {
            for auto in [false, true] {
                check(&fmt(|s| net_rate(s, r, unit, auto)));
            }
        }
        let c = ceil_125(r);
        prop_assert!(!c.is_nan());
        if r > 0.0 {
            prop_assert!(c >= r);
        }
        let q = ceil_pow2(n);
        prop_assert!(q.is_power_of_two());
    }
}

#[test]
fn count_grouping() {
    let cases = [
        (0, "0"),
        (999, "999"),
        (1000, "1\u{2009}000"),
        (12345, "12\u{2009}345"),
        (1_234_567, "1\u{2009}234\u{2009}567"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| count(s, n)), want, "count({n})");
    }
}

#[test]
fn cpu_time_forms() {
    let cases = [
        (0, "0:00:00"),
        (43_424, "12:03:44"),
        (3_599_999, "999:59:59"),
        (3_600_000, "41d 16:00"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| cpu_time(s, n)), want, "cpu_time({n})");
    }
}

#[test]
fn uptime_forms() {
    let cases = [
        (0, "0h 0m 0s"),
        (2_295, "0h 38m 15s"),
        (9_495, "2h 38m 15s"),
        (95_895, "1 day, 2h 38m 15s"),
        (182_295, "2 days, 2h 38m 15s"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| uptime(s, n)), want, "uptime({n})");
    }
}

#[test]
fn short_age_forms() {
    let cases = [
        (0, "0s"),
        (5, "5s"),
        (725, "12m 5s"),
        (15_120, "4h 12m"),
        (100_800, "1d 4h"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| short_age(s, n)), want, "short_age({n})");
    }
}

#[test]
fn start_time_and_date_time() {
    let t = LocalDateTime {
        year: 2026,
        month: 10,
        day: 3,
        hour: 9,
        minute: 5,
        second: 7,
    };
    assert_eq!(fmt(|s| start_time(s, t, true)), "09:05:07");
    assert_eq!(fmt(|s| start_time(s, t, false)), "2026-10-03 09:05");
    let t = LocalDateTime {
        year: 2026,
        month: 1,
        day: 2,
        hour: 3,
        minute: 4,
        second: 5,
    };
    assert_eq!(fmt(|s| date_time(s, t)), "2026-01-02 03:04:05");
}

#[test]
fn freq_temp_power_forms() {
    assert_eq!(fmt(|s| mhz(s, 4631.4)), "4631 MHz");
    assert_eq!(fmt(|s| mhz_pair(s, 1905.0, 3105.0)), "1905 / 3105 MHz");
    assert_eq!(fmt(|s| celsius(s, 51.6)), "52 \u{b0}C");
    assert_eq!(fmt(|s| watts(s, 31.94)), "31.9 W");
    assert_eq!(fmt(|s| watts(s, 0.0)), "0 W");
    assert_eq!(fmt(|s| watts_pair(s, 31.9, 300.0)), "31.9 / 300 W");
}

#[test]
fn cache_size_forms() {
    let cases = [
        (512, "512 KiB"),
        (1023, "1023 KiB"),
        (8192, "8 MiB"),
        (98_304, "96 MiB"),
        (1536, "1.5 MiB"),
    ];
    for (n, want) in cases {
        assert_eq!(fmt(|s| cache_size(s, n)), want, "cache_size({n})");
    }
}

#[test]
fn socket_addr_forms() {
    let any4 = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let any6 = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let link: IpAddr = "fe80::1".parse().unwrap();
    let web: IpAddr = "93.184.216.34".parse().unwrap();
    let cases = [
        (any4, 22, None, AddrSide::Local, "0.0.0.0:22"),
        (any4, 0, None, AddrSide::Local, "0.0.0.0:*"),
        (any6, 0, None, AddrSide::Local, "[::]:*"),
        (
            link,
            5353,
            Some("eno1"),
            AddrSide::Local,
            "[fe80::1%eno1]:5353",
        ),
        (any4, 0, None, AddrSide::Foreign, "*:*"),
        (any6, 0, None, AddrSide::Foreign, "*:*"),
        (web, 443, None, AddrSide::Foreign, "93.184.216.34:443"),
    ];
    for (ip, port, scope, side, want) in cases {
        assert_eq!(
            fmt(|s| socket_addr(s, ip, port, scope, side)),
            want,
            "socket_addr({ip}, {port}, {scope:?}, {side:?})"
        );
    }
}

#[test]
fn response_ms_forms() {
    assert_eq!(fmt(|s| response_ms(s, 0.4213)), "0.42 ms");
    assert_eq!(fmt(|s| response_ms(s, 0.0)), "0 ms");
}

prop_compose! {
    fn any_local_date_time()(
        year in any::<i32>(),
        month in any::<u8>(),
        day in any::<u8>(),
        hour in any::<u8>(),
        minute in any::<u8>(),
        second in any::<u8>(),
    ) -> LocalDateTime {
        LocalDateTime { year, month, day, hour, minute, second }
    }
}

proptest! {
    #[test]
    fn time_formatters_never_panic(
        n in any::<u64>(),
        t in any_local_date_time(),
        today in any::<bool>(),
    ) {
        check(&fmt(|s| cpu_time(s, n)));
        check(&fmt(|s| uptime(s, n)));
        check(&fmt(|s| short_age(s, n)));
        check(&fmt(|s| start_time(s, t, today)));
        check(&fmt(|s| date_time(s, t)));
    }
}
