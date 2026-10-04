//! Tray graph icon rasterizer tests (tasklens-wox.5).

use proptest::prelude::*;
use tl_model::series::{Sample, SampleFlags};
use tl_tray::raster::render;

const S: u64 = 1_000_000_000;
/// 18 s range at 22 px gives 1 s buckets.
const RANGE: u64 = 18 * S;
const RGB: [u8; 3] = [10, 200, 30];
/// Fill alpha: 0.55 of the line colour.
const FILL: u8 = 140;

fn px(out: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * size + x) * 4) as usize;
    [out[i], out[i + 1], out[i + 2], out[i + 3]]
}

fn alpha(out: &[u8], size: u32, x: u32, y: u32) -> u8 {
    px(out, size, x, y)[0]
}

fn draw(samples: &[Sample], now_ns: u64, range_ns: u64, max: f32, size: u32) -> Vec<u8> {
    let mut out = Vec::new();
    render(samples, now_ns, range_ns, max, RGB, size, &mut out);
    out
}

/// One valid sample per second for the last 18 s, values 10, 20, .. wrapping at 100.
fn ramp(now_ns: u64) -> Vec<Sample> {
    (0..18u64)
        .map(|i| Sample::valid(now_ns - (17 - i) * S, ((i % 10 + 1) * 10) as f32))
        .collect()
}

#[test]
fn raster_22_has_transparent_margin() {
    let now = 100 * S;
    let mut samples = ramp(now);
    samples.push(Sample::valid(now, 100.0));
    let out = draw(&samples, now, RANGE, 100.0, 22);
    assert_eq!(out.len(), 1936);
    for i in 0..22 {
        for (x, y) in [(i, 0), (i, 21), (0, i), (21, i)] {
            assert_eq!(alpha(&out, 22, x, y), 0, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn raster_value_bar_height() {
    let now = 100 * S;
    let out = draw(&[Sample::valid(now, 50.0)], now, RANGE, 100.0, 22);
    for x in [19, 20] {
        for y in 11..=20 {
            assert_eq!(px(&out, 22, x, y), [255, 10, 200, 30], "bar ({x}, {y})");
        }
        for y in 1..=10 {
            assert_eq!(alpha(&out, 22, x, y), 0, "above bar ({x}, {y})");
        }
    }
}

#[test]
fn raster_invalid_value_draws_baseline() {
    let now = 100 * S;
    let samples = [Sample::valid(now - S, 50.0), Sample::gap(now)];
    let out = draw(&samples, now, RANGE, 100.0, 22);
    for x in [19, 20] {
        assert_eq!(px(&out, 22, x, 20), [255, 10, 200, 30], "baseline ({x})");
        for y in 1..=19 {
            assert_eq!(alpha(&out, 22, x, y), 0, "above baseline ({x}, {y})");
        }
    }
    // Empty input also draws only the baseline.
    let out = draw(&[], now, RANGE, 100.0, 22);
    for x in [19, 20] {
        assert_eq!(alpha(&out, 22, x, 20), 255);
        assert_eq!(alpha(&out, 22, x, 19), 0);
    }
}

#[test]
fn raster_identical_input_identical_bytes() {
    let now = 100 * S;
    let samples = ramp(now);
    let a = draw(&samples, now, RANGE, 80.0, 22);
    let mut b = vec![7u8; 5];
    render(&samples, now, RANGE, 80.0, RGB, 22, &mut b);
    assert_eq!(a, b);
}

#[test]
fn raster_gap_columns_transparent() {
    let now = 100 * S;
    let samples = [
        Sample::valid(95 * S, 60.0),
        Sample::gap(96 * S),
        Sample::gap(97 * S),
        Sample::gap(98 * S),
        Sample::valid(99 * S, 60.0),
        Sample::valid(100 * S, 60.0),
    ];
    let out = draw(&samples, now, RANGE, 100.0, 22);
    // Bucket 100 is column 18, so buckets 96..=98 are columns 14..=16.
    for x in 14..=16 {
        for y in 0..22 {
            assert_eq!(alpha(&out, 22, x, y), 0, "gap ({x}, {y})");
        }
    }
    for x in [13, 17, 18] {
        assert_eq!(alpha(&out, 22, x, 20), FILL, "drawn column {x}");
    }
}

#[test]
fn raster_44_is_2x_of_22_layout() {
    let now = 100 * S;
    // The 44 px icon has 36 half-width buckets over the same range, so every column gets the
    // same value. Ratios are multiples of 0.05, so 20 * r and 40 * r round the same way.
    let mut samples: Vec<Sample> = (0..72u64)
        .map(|i| Sample::valid(now - 18 * S + (i + 1) * S / 4, 50.0))
        .collect();
    samples.push(Sample::valid(now, 35.0));
    let small = draw(&samples, now, RANGE, 100.0, 22);
    let big = draw(&samples, now, RANGE, 100.0, 44);
    assert_eq!(big.len(), 44 * 44 * 4);
    for y in 0..44 {
        for x in 0..44 {
            assert_eq!(
                px(&big, 44, x, y),
                px(&small, 22, x / 2, y / 2),
                "pixel ({x}, {y})"
            );
        }
    }
    // Margin 2 px.
    for i in 0..44 {
        for (x, y) in [
            (i, 0),
            (i, 1),
            (i, 42),
            (i, 43),
            (0, i),
            (1, i),
            (42, i),
            (43, i),
        ] {
            assert_eq!(alpha(&big, 44, x, y), 0, "margin ({x}, {y})");
        }
    }
    // 36 history columns, each drawn at the bottom inner row.
    for x in 2..=37 {
        assert_eq!(alpha(&big, 44, x, 41), FILL, "history column {x}");
    }
    // 4 px bar: 35 of 100 is 14 rows of 40.
    for x in 38..=41 {
        assert_eq!(alpha(&big, 44, x, 41), 255, "bar column {x}");
        assert_eq!(alpha(&big, 44, x, 28), 255, "bar top {x}");
        assert_eq!(alpha(&big, 44, x, 27), 0, "above bar {x}");
    }
}

#[test]
fn raster_buckets_use_absolute_time() {
    let samples = [Sample::valid(50 * S + S / 5, 70.0)];
    let a = draw(&samples, 55 * S, RANGE, 100.0, 22);
    let b = draw(&samples, 55 * S + 9 * S / 10, RANGE, 100.0, 22);
    assert_eq!(a, b);
    // Bucket 50 is 5 buckets before bucket 55, so it is column 13.
    assert_eq!(alpha(&a, 22, 13, 20), FILL);
    assert_eq!(alpha(&a, 22, 12, 20), 0);
    assert_eq!(alpha(&a, 22, 14, 20), 0);
}

#[test]
fn raster_full_column_line_and_fill() {
    let now = 100 * S;
    let out = draw(&[Sample::valid(now, 100.0)], now, RANGE, 100.0, 22);
    assert_eq!(px(&out, 22, 18, 1), [255, 10, 200, 30]);
    for y in 2..=20 {
        let p = px(&out, 22, 18, y);
        assert!(p[0].abs_diff(FILL) <= 2, "fill alpha at row {y}: {}", p[0]);
        assert_eq!(&p[1..], &RGB, "fill colour at row {y}");
    }
}

#[test]
fn raster_bad_max_draws_baseline_only() {
    let now = 100 * S;
    let samples = ramp(now);
    for max in [0.0, -5.0, f32::NAN, f32::INFINITY] {
        let out = draw(&samples, now, RANGE, max, 22);
        for y in 0..22 {
            for x in 0..22 {
                let want = if (x == 19 || x == 20) && y == 20 {
                    255
                } else {
                    0
                };
                assert_eq!(alpha(&out, 22, x, y), want, "max {max} ({x}, {y})");
            }
        }
    }
}

fn sample_strategy() -> impl Strategy<Value = Sample> {
    (
        0u64..200 * S,
        prop_oneof![
            any::<f32>(),
            Just(f32::NAN),
            Just(f32::INFINITY),
            -10.0f32..1e12
        ],
        any::<bool>(),
    )
        .prop_map(|(t_ns, value, valid)| Sample {
            t_ns,
            value,
            flags: if valid {
                SampleFlags::VALID
            } else {
                SampleFlags::empty()
            },
        })
}

proptest! {
    #[test]
    fn prop_render_never_panics(
        samples in prop::collection::vec(sample_strategy(), 0..64),
        now_ns in 0u64..250 * S,
        range_ns in 0u64..100 * S,
        max in -1.0f32..1e12,
        size in prop_oneof![Just(22u32), Just(44u32)],
    ) {
        let mut out = Vec::new();
        render(&samples, now_ns, range_ns, max, RGB, size, &mut out);
        prop_assert_eq!(out.len(), (size * size * 4) as usize);
    }
}
