//! Formatting cases for `tl_model::units`, canonical table (d).

use proptest::prelude::*;
use tl_model::units::{
    NetUnit, bytes, bytes_of, ceil_125, ceil_pow2, mib_whole, net_rate, percent, percent_2dp,
    rate_iec_auto, rate_mib_fixed,
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
