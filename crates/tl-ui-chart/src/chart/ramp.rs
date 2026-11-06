//! The value colour ramp of power draw charts (tasklens-1vwxr).
//!
//! A power draw chart fills its area and draws its line with a vertical gradient tied to the
//! value axis: the theme's `power-low` at 0 W (the plot bottom), `power-mid` at half the axis
//! and `power-high` at the axis maximum (the plot top). The same wattage gets the same colour
//! while the axis stays the same.
//!
//! GPUI paths take a two-stop linear gradient over the path's bounds, and those bounds also clip
//! the path. So each ramped path is painted twice with the same vertices: once with its bounds
//! set to the lower half of the plot (low to mid) and once to the upper half (mid to high). A
//! half the path does not reach is skipped. This costs one extra vertex copy per ramped path
//! per frame and no geometry rebuild; stepped bands would cost 8 or more paints.

use gpui_kit::{
    Background, Bounds, Hsla, Pixels, Rgba, linear_color_stop, linear_gradient, point, px, size,
};

use super::{ChartSpec, ValueFormat, YScale};
use tl_ui_base::theme::TlTheme;

/// Alpha of the ramp colours in a fill. The line on top uses them at full alpha. A part
/// transparent fill keeps the muted fill look of the other charts in dark themes and gives a
/// pastel fill in light themes.
pub const FILL_ALPHA: f32 = 0.45;

/// Three colours from the bottom of the value axis to its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueRamp {
    pub low: Hsla,
    pub mid: Hsla,
    pub high: Hsla,
}

impl ValueRamp {
    /// The theme's power draw ramp.
    pub fn power(theme: &TlTheme) -> Self {
        let [low, mid, high] = theme.graph.power_ramp();
        Self { low, mid, high }
    }

    /// The ramp of a chart, `None` when the chart does not use one. Power draw charts do: the
    /// value format is watts and the axis starts at 0. A battery's charge and discharge chart
    /// (`YScale::Symmetric`) and stacked charts keep their flat colours.
    pub fn of_chart(fmt: ValueFormat, spec: &ChartSpec, theme: &TlTheme) -> Option<Self> {
        let ramped = fmt == ValueFormat::Watts
            && !spec.stack
            && !matches!(spec.y, YScale::Symmetric | YScale::Stacked(_));
        ramped.then(|| Self::power(theme))
    }

    /// The colour at `frac` of the axis: 0 at the bottom, 1 at the top, clamped. Mixed in
    /// sRGB, as the GPUI gradient mixes its stops.
    pub fn at(&self, frac: f32) -> Hsla {
        let f = if frac.is_nan() {
            0.0
        } else {
            frac.clamp(0.0, 1.0)
        };
        if f <= 0.5 {
            mix(self.low, self.mid, f * 2.0)
        } else {
            mix(self.mid, self.high, f * 2.0 - 1.0)
        }
    }

    /// The colour of `value` on an axis from `y_min` to `y_max`.
    pub fn at_value(&self, value: f32, y_min: f32, y_max: f32) -> Hsla {
        let span = y_max - y_min;
        if span <= 0.0 {
            return self.low;
        }
        self.at((value - y_min) / span)
    }

    /// The two gradient bands over `plot` (window logical px): the lower half from `low` to
    /// `mid` and the upper half from `mid` to `high`, every colour with alpha `alpha`. The
    /// split is on a whole physical px at `scale_factor`, so the two halves share no pixel row.
    pub fn bands(&self, plot: Bounds<Pixels>, scale_factor: f32, alpha: f32) -> [RampBand; 2] {
        let sf = scale_factor.max(f32::MIN_POSITIVE);
        let (top, bottom) = (f32::from(plot.top()), f32::from(plot.bottom()));
        let split = (((top + bottom) * 0.5 * sf).round() / sf).clamp(top, bottom);
        let x = plot.left();
        let w = plot.size.width;
        let a = |c: Hsla| Hsla {
            a: c.a * alpha,
            ..c
        };
        [
            RampBand {
                bounds: Bounds::new(point(x, px(split)), size(w, px(bottom - split))),
                from: a(self.low),
                to: a(self.mid),
            },
            RampBand {
                bounds: Bounds::new(point(x, px(top)), size(w, px(split - top))),
                from: a(self.mid),
                to: a(self.high),
            },
        ]
    }
}

/// One half of the ramp: a gradient from `from` at the bottom of `bounds` to `to` at its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RampBand {
    pub bounds: Bounds<Pixels>,
    pub from: Hsla,
    pub to: Hsla,
}

impl RampBand {
    /// The GPUI background: angle 0 runs from the bottom of the bounds to the top.
    pub fn background(&self) -> Background {
        linear_gradient(
            0.0,
            linear_color_stop(self.from, 0.0),
            linear_color_stop(self.to, 1.0),
        )
    }

    /// True when a path with bounds `path` (window logical px) reaches into this band.
    pub fn reaches(&self, path: &Bounds<Pixels>) -> bool {
        path.top() < self.bounds.bottom() && path.bottom() > self.bounds.top()
    }
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rgba {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;

    fn ramp() -> ValueRamp {
        ValueRamp {
            low: rgb(0x00A0A0).into(),
            mid: rgb(0xF0D000).into(),
            high: rgb(0xF04000).into(),
        }
    }

    fn rgba(c: Hsla) -> Rgba {
        c.into()
    }

    fn close(a: Hsla, b: Hsla) -> bool {
        let (a, b) = (rgba(a), rgba(b));
        (a.r - b.r).abs() < 0.01
            && (a.g - b.g).abs() < 0.01
            && (a.b - b.b).abs() < 0.01
            && (a.a - b.a).abs() < 0.01
    }

    #[test]
    fn stops_at_ends_and_middle() {
        let r = ramp();
        assert!(close(r.at(0.0), r.low));
        assert!(close(r.at(0.5), r.mid));
        assert!(close(r.at(1.0), r.high));
        assert!(close(r.at(-3.0), r.low), "clamped below");
        assert!(close(r.at(7.0), r.high), "clamped above");
        assert!(close(r.at(f32::NAN), r.low));
    }

    /// 180 W on a 200 W axis is near the warm end, 20 W near the cool end; on a 400 W axis
    /// 180 W moves down towards the middle (the stops follow the axis max).
    #[test]
    fn value_follows_axis_max() {
        let r = ramp();
        let hot = rgba(r.at_value(180.0, 0.0, 200.0));
        let cool = rgba(r.at_value(20.0, 0.0, 200.0));
        assert!(hot.r > 0.9 && hot.b < 0.05, "{hot:?}");
        assert!(cool.r < 0.25 && cool.b > 0.5, "{cool:?}");
        assert!(close(r.at_value(180.0, 0.0, 400.0), r.at(0.45)));
        assert!(close(r.at_value(5.0, 0.0, 0.0), r.low), "empty axis");
    }

    #[test]
    fn bands_split_plot_on_whole_px() {
        let r = ramp();
        let plot = Bounds::new(point(px(10.), px(20.5)), size(px(300.), px(101.)));
        let [lower, upper] = r.bands(plot, 2.0, 0.5);
        assert_eq!(lower.bounds.bottom(), plot.bottom());
        assert_eq!(upper.bounds.top(), plot.top());
        assert_eq!(lower.bounds.top(), upper.bounds.bottom(), "the halves meet");
        let split = f32::from(lower.bounds.top()) * 2.0;
        assert_eq!(split, split.round(), "split on a whole physical px");
        assert_eq!(
            (lower.bounds.left(), lower.bounds.size.width),
            (px(10.), px(300.))
        );
        assert_eq!(lower.from, Hsla { a: 0.5, ..r.low });
        assert_eq!(lower.to, Hsla { a: 0.5, ..r.mid });
        assert_eq!(upper.from, Hsla { a: 0.5, ..r.mid });
        assert_eq!(upper.to, Hsla { a: 0.5, ..r.high });

        let low_path = Bounds::new(point(px(10.), px(100.)), size(px(300.), px(21.5)));
        assert!(lower.reaches(&low_path));
        assert!(!upper.reaches(&low_path), "a low path skips the upper band");
    }
}
