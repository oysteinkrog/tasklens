//! Tray graph icon rasterizer (UI-PRF-050, ADR-010 Tray).
//!
//! Layout at 22 px, all values doubled at 44 px: a 1 px transparent margin, 18 history columns
//! and a 2 px value bar on the right. Each history column is one absolute-time bucket drawn as
//! fill plus a 1 px line on top. The layout is drawn at 2x and box-filtered down 2 x 2.

use std::cell::RefCell;

use tl_model::series::Sample;

/// Base icon size the layout is defined at.
const BASE: u32 = 22;
/// Fill alpha: 0.55 of the line colour.
const FILL_ALPHA: u8 = 140;

thread_local! {
    /// 2x drawing buffer, `[A, R, G, B]` straight alpha. Reused across calls.
    static SUPER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// Per-column bucket values. NaN marks a gap.
    static BUCKETS: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// Draws one tray graph icon. `size` is 22 or 44. `out` is resized to size*size*4 and overwritten.
/// Pure CPU; no allocation after the first call for a given size.
pub fn render(
    samples: &[Sample],
    now_ns: u64,
    range_ns: u64,
    max: f32,
    line_rgb: [u8; 3],
    size: u32,
    out: &mut Vec<u8>,
) {
    let size = size as usize;
    out.clear();
    out.resize(size * size * 4, 0);

    let layout = Layout::new(size);
    let [r, g, b] = line_rgb;
    let line = [255, r, g, b];
    let fill = [FILL_ALPHA, r, g, b];
    let max_ok = max.is_finite() && max > 0.0;

    SUPER.with_borrow_mut(|sup| {
        BUCKETS.with_borrow_mut(|buckets| {
            let w = size * 2;
            sup.clear();
            sup.resize(w * w * 4, 0);

            fill_buckets(samples, now_ns, range_ns, layout.cols, buckets);

            let bottom = layout.margin + layout.inner_h;
            for (i, &value) in buckets.iter().enumerate() {
                let h = height(value, max, max_ok, layout.inner_h);
                if h == 0 {
                    continue;
                }
                let x = layout.margin + i;
                let top = bottom - h;
                let line_end = top + layout.unit.min(h);
                fill_rect(sup, w, x, x + 1, top, line_end, line);
                fill_rect(sup, w, x, x + 1, line_end, bottom, fill);
            }

            let bar_x = layout.margin + layout.cols;
            // Baseline when the current value is invalid, missing or has no usable scale.
            let bar_h = match newest(samples) {
                Some(s) if s.is_valid() && max_ok => height(s.value, max, max_ok, layout.inner_h),
                _ => layout.unit.min(layout.inner_h),
            };
            fill_rect(
                sup,
                w,
                bar_x,
                bar_x + layout.bar_w,
                bottom - bar_h,
                bottom,
                line,
            );

            downsample(sup, w, out, size);
        });
    });
}

/// Layout in target pixels.
struct Layout {
    /// 1 at 22 px, 2 at 44 px.
    unit: usize,
    margin: usize,
    inner_h: usize,
    cols: usize,
    bar_w: usize,
}

impl Layout {
    fn new(size: usize) -> Layout {
        let unit = (size / BASE as usize).max(1);
        let margin = unit.min(size / 2);
        let inner = size - 2 * margin;
        let bar_w = (2 * unit).min(inner);
        Layout {
            unit,
            margin,
            inner_h: inner,
            cols: inner - bar_w,
            bar_w,
        }
    }
}

/// Fills `buckets` with one value per history column, oldest first. NaN marks a gap.
fn fill_buckets(
    samples: &[Sample],
    now_ns: u64,
    range_ns: u64,
    cols: usize,
    buckets: &mut Vec<f32>,
) {
    buckets.clear();
    buckets.resize(cols, f32::NAN);
    if cols == 0 {
        return;
    }
    let bucket_ns = (range_ns / cols as u64).max(1);
    let newest = now_ns / bucket_ns;
    for s in samples.iter().filter(|s| s.is_valid()) {
        let b = s.t_ns / bucket_ns;
        if b > newest {
            continue;
        }
        let age = newest - b;
        if age >= cols as u64 {
            continue;
        }
        let slot = &mut buckets[cols - 1 - age as usize];
        // f32::max ignores NaN, so a NaN sample never hides a real value.
        *slot = if slot.is_nan() {
            s.value
        } else {
            slot.max(s.value)
        };
    }
}

/// The sample with the largest `t_ns`; the later one wins a tie.
fn newest(samples: &[Sample]) -> Option<&Sample> {
    samples.iter().max_by_key(|s| s.t_ns)
}

/// `round(inner_h * clamp(value / max, 0, 1))`; 0 for a NaN value or an unusable `max`.
fn height(value: f32, max: f32, max_ok: bool, inner_h: usize) -> usize {
    if !max_ok {
        return 0;
    }
    let ratio = value / max;
    if ratio.is_nan() {
        return 0;
    }
    (inner_h as f32 * ratio.clamp(0.0, 1.0)).round() as usize
}

/// Fills target-pixel rect `[x0, x1) x [y0, y1)` in the 2x buffer of width `w`.
fn fill_rect(buf: &mut [u8], w: usize, x0: usize, x1: usize, y0: usize, y1: usize, px: [u8; 4]) {
    for y in y0 * 2..y1 * 2 {
        let row = y * w * 4;
        for x in x0 * 2..x1 * 2 {
            buf[row + x * 4..row + x * 4 + 4].copy_from_slice(&px);
        }
    }
}

/// Box-filters each 2 x 2 block: averages in premultiplied form, stores straight alpha.
fn downsample(sup: &[u8], w: usize, out: &mut [u8], size: usize) {
    for y in 0..size {
        for x in 0..size {
            let mut sum_a = 0u32;
            let mut sum_c = [0u32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let i = ((y * 2 + dy) * w + x * 2 + dx) * 4;
                let a = u32::from(sup[i]);
                sum_a += a;
                for (c, sum) in sum_c.iter_mut().enumerate() {
                    *sum += u32::from(sup[i + 1 + c]) * a;
                }
            }
            let o = (y * size + x) * 4;
            if sum_a == 0 {
                out[o..o + 4].fill(0);
                continue;
            }
            out[o] = ((sum_a + 2) / 4) as u8;
            for c in 0..3 {
                out[o + 1 + c] = ((sum_c[c] + sum_a / 2) / sum_a) as u8;
            }
        }
    }
}
