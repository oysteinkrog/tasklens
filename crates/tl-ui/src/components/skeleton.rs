//! Loading placeholders: skeleton rows for tables and skeleton blocks for key/value grids and
//! Information cards (UI-FND-024, APP-059, R-060, SYS-016).
//!
//! Rules:
//! - A tab shows its skeleton until its first data in the session, then never again. Owners keep
//!   one `SkeletonGate` per tab. A later empty result is the empty state, not loading.
//! - The owner replaces the skeleton with the data in the same frame the data arrives. No fade.
//! - Never put a skeleton inside `uniform_list`.
//! - The Processes tab has no skeleton (PROC-021).
//! - While visible, the skeleton pulses opacity 0.5 to 1.0 over 2 s (kit `Skeleton`), so it
//!   requests frames. Under reduced motion it is static at opacity 0.7 and requests none.
//! - The bars are decorative. Owners show real text once the data arrives.

use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Div, Hsla, IntoElement, Length, ParentElement, Pixels, RenderOnce, Styled,
    Window, div, px, relative,
};
use smallvec::SmallVec;

use crate::theme::metrics;

/// Bar widths in a skeleton row, as fractions of the column width. Column `c` of row `r` uses
/// entry `(r + c) % 5`, so each row starts one step further along.
pub const ROW_PATTERN: [f32; 5] = [0.6, 0.45, 0.8, 0.5, 0.7];

/// Line widths in a skeleton block, as fractions of the block width. Line `i` uses entry `i % 4`.
pub const BLOCK_PATTERN: [f32; 4] = [0.9, 0.7, 0.8, 0.5];

/// The most skeleton rows a table shows.
pub const MAX_ROWS: usize = 12;

/// Lines in a skeleton block unless the owner sets another count.
pub const BLOCK_LINES: usize = 8;

/// Bar and line height before the UI scale.
const BAR_HEIGHT: f32 = 10.0;

/// Opacity of every bar under reduced motion.
pub const STATIC_OPACITY: f32 = 0.7;

/// Rows to draw for a table with room for `visible_rows` rows: at most 12.
pub fn skeleton_row_count(visible_rows: usize) -> usize {
    visible_rows.min(MAX_ROWS)
}

/// Width of the bar in column `col` of row `row`, for a column `column` wide.
pub fn bar_width(column: Pixels, row: usize, col: usize) -> Pixels {
    column * ROW_PATTERN[(row + col) % ROW_PATTERN.len()]
}

/// Decides whether a tab shows its skeleton. True until the first call with data, then false
/// for the rest of the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkeletonGate {
    seen_data: bool,
}

impl SkeletonGate {
    pub fn show(&mut self, has_data: bool) -> bool {
        self.seen_data |= has_data;
        !self.seen_data
    }
}

/// Fake table rows: `rows` rows of `row_height`, one rounded bar per column.
#[derive(IntoElement)]
pub struct TlSkeletonRows {
    rows: usize,
    columns: SmallVec<[Pixels; 8]>,
}

impl TlSkeletonRows {
    /// `rows` is usually `skeleton_row_count(visible rows)`. `columns` are the column widths.
    pub fn new(rows: usize, columns: impl IntoIterator<Item = Pixels>) -> Self {
        Self {
            rows,
            columns: columns.into_iter().collect(),
        }
    }
}

impl RenderOnce for TlSkeletonRows {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let height = px(BAR_HEIGHT * m.ui_scale);
        v_flex().children((0..self.rows).map(|row| {
            h_flex()
                .h(m.row_height)
                .children(self.columns.iter().enumerate().map(|(col, &width)| {
                    div()
                        .flex_none()
                        .w(width)
                        .pl(m.sp[4])
                        .overflow_hidden()
                        .child(bar(
                            bar_width(width, row, col).into(),
                            height,
                            m.radius_small,
                            cx,
                        ))
                }))
        }))
    }
}

/// Fake text lines for key/value grids and Information cards.
#[derive(IntoElement)]
pub struct TlSkeletonBlock {
    lines: usize,
}

impl Default for TlSkeletonBlock {
    fn default() -> Self {
        Self { lines: BLOCK_LINES }
    }
}

impl TlSkeletonBlock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lines(mut self, lines: usize) -> Self {
        self.lines = lines;
        self
    }
}

impl RenderOnce for TlSkeletonBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let height = px(BAR_HEIGHT * m.ui_scale);
        v_flex().gap(m.sp[3]).children((0..self.lines).map(|i| {
            let fraction = BLOCK_PATTERN[i % BLOCK_PATTERN.len()];
            bar(relative(fraction).into(), height, m.radius_small, cx)
        }))
    }
}

/// One bar: the pulsing kit `Skeleton`, or a static bar under reduced motion. The kit
/// animation shows its start state (opacity 1.0) under reduced motion, so it is not used then.
fn bar(width: Length, height: Pixels, radius: Pixels, cx: &App) -> AnyElement {
    if cx.reduce_motion() {
        static_bar(width, height, radius, cx.theme().skeleton).into_any_element()
    } else {
        Skeleton::new()
            .w(width)
            .h(height)
            .rounded(radius)
            .into_any_element()
    }
}

fn static_bar(width: Length, height: Pixels, radius: Pixels, color: Hsla) -> Div {
    div()
        .w(width)
        .h(height)
        .rounded(radius)
        .bg(color)
        .opacity(STATIC_OPACITY)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Context, Render, TestAppContext, size};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .child(TlSkeletonRows::new(
                    skeleton_row_count(30),
                    [px(100.), px(100.), px(100.)],
                ))
                .child(TlSkeletonBlock::new())
        }
    }

    fn setup(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
        });
    }

    #[test]
    fn shown_once_per_tab() {
        let mut gate = SkeletonGate::default();
        assert!(gate.show(false), "empty before first data: loading");
        assert!(!gate.show(true), "data arrived");
        assert!(!gate.show(false), "empty after data is the empty state");
    }

    #[test]
    fn row_cap_12() {
        assert_eq!(skeleton_row_count(30), 12);
        assert_eq!(skeleton_row_count(5), 5);
    }

    #[test]
    fn width_pattern() {
        let widths: Vec<f32> = (0..3)
            .map(|col| f32::from(bar_width(px(100.), 0, col)))
            .collect();
        for (got, want) in widths.iter().zip([60.0, 45.0, 80.0]) {
            assert!((got - want).abs() < 1e-3, "{widths:?}");
        }
        // Row 1 starts one step along the pattern.
        assert!((f32::from(bar_width(px(100.), 1, 0)) - 45.0).abs() < 1e-3);
    }

    #[gpui_kit::test]
    fn reduced_motion_static(cx: &mut TestAppContext) {
        setup(cx);
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.open_window(size(px(400.), px(600.)), |_, _| Host);
        cx.run_until_parked();
        let frames = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert_eq!(frames, 0, "reduced motion requests no frame");

        let mut b = static_bar(px(60.).into(), px(10.), px(4.), gpui_kit::black());
        assert_eq!(b.style().opacity, Some(STATIC_OPACITY));
        assert_eq!(STATIC_OPACITY, 0.7);
    }

    #[gpui_kit::test]
    fn pulses_without_reduced_motion(cx: &mut TestAppContext) {
        setup(cx);
        let window = cx.open_window(size(px(400.), px(600.)), |_, _| Host);
        cx.run_until_parked();
        let frames = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        assert!(frames > 0, "the pulse requests frames");
    }
}
