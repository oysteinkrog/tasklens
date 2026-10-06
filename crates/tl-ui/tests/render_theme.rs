//! Theme render goldens (E17-012, ADR-013, UI-FND-030): a swatch view of every theme colour and
//! two kit widgets, rendered on lavapipe for both built-in themes at `ui_scale` 1.0 and 1.1 and
//! compared with `tests/golden/theme_<name>_<scale>.png`. `TL_UPDATE_GOLDEN=1` rewrites them.
//!
//! Run with `cargo test -p tl-ui --features render-tests,test-hooks --test render_theme`. The
//! render helpers live in `tl_ui::testing`, which needs `test-hooks` (R-291).

#![cfg(feature = "render-tests")]

#[cfg(not(feature = "test-hooks"))]
compile_error!("render_theme needs --features render-tests,test-hooks (R-291)");

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::{
    AppContext, Context, Entity, Hsla, IntoElement, ParentElement, Render, Styled, Window, div, px,
    size,
};
use tl_ui::testing::render::{RenderHarness, assert_golden, run_on_lavapipe};
use tl_ui::text::fonts::UI_FAMILY;
use tl_ui::theme::{GraphPalette, StatusColors, ThemeSlot, current, metrics};

/// Window size at `ui_scale` 1.0, in logical px.
const WIDTH: f32 = 480.;
const HEIGHT: f32 = 760.;
/// Swatch block size at `ui_scale` 1.0.
const BLOCK_W: f32 = 120.;
const BLOCK_H: f32 = 20.;

/// The 13 graph palette colours in field order.
fn graph_colors(g: &GraphPalette) -> [Hsla; 13] {
    [
        g.cpu_fill,
        g.cpu_line,
        g.kernel_line,
        g.mem_fill,
        g.mem_line,
        g.gpu_fill,
        g.gpu_line,
        g.disk_fill,
        g.disk_line,
        g.net_fill,
        g.net_line,
        g.grid,
        g.chart_bg,
    ]
}

/// The 4 status colours in field order.
fn status_colors(s: &StatusColors) -> [Hsla; 4] {
    [s.success, s.warning, s.danger, s.info]
}

/// One row per `ThemeSlot`, then the graph palette and status blocks, then a primary kit
/// `Button` and a kit `Input`. Built only from `theme::current()` and the kit `Theme`.
struct ThemeSwatches {
    input: Entity<InputState>,
}

impl ThemeSwatches {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter processes"));
        Self { input }
    }
}

impl Render for ThemeSwatches {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let block = |c: Hsla| div().w(px(BLOCK_W * s)).h(px(BLOCK_H * s)).bg(c);
        let blocks = |colors: &[Hsla]| {
            div()
                .mt(m.sp[4])
                .flex()
                .flex_row()
                .flex_wrap()
                .children(colors.iter().map(|&c| block(c)))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.window_bg)
            .font_family(UI_FAMILY)
            .text_size(m.text.sm)
            .text_color(t.text_primary)
            .children(ThemeSlot::ALL.iter().map(|&slot| {
                div()
                    .h(px(BLOCK_H * s))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(m.sp[4])
                    .child(block(slot.get(&t)))
                    .child(slot.label())
            }))
            .child(blocks(&graph_colors(&t.graph)))
            .child(blocks(&status_colors(&t.status)))
            .child(
                div()
                    .mt(m.sp[4])
                    .px(m.sp[4])
                    .flex()
                    .flex_col()
                    .gap(m.sp[4])
                    .child(div().child(Button::new("primary").primary().label("Apply")))
                    .child(div().w(px(240. * s)).child(Input::new(&self.input))),
            )
    }
}

/// Activates `theme` with `ActivateSource::User`, sets `ui_scale`, renders the swatches and
/// compares with the golden `name`.
fn check(theme: &str, scale: f32, name: &str) {
    let mut harness = RenderHarness::new();
    harness.set_theme(theme);
    harness.set_scale(scale);
    let img = harness.render(size(px(WIDTH), px(HEIGHT)), |window, cx| {
        cx.new(|cx| ThemeSwatches::new(window, cx))
    });
    assert_golden(name, &img);
}

#[test]
fn default_dark_scale_1_0() {
    run_on_lavapipe("default_dark_scale_1_0", || {
        check("Default Dark", 1.0, "theme_default_dark_1_0")
    });
}

#[test]
fn default_dark_scale_1_1() {
    run_on_lavapipe("default_dark_scale_1_1", || {
        check("Default Dark", 1.1, "theme_default_dark_1_1")
    });
}

#[test]
fn default_light_scale_1_0() {
    run_on_lavapipe("default_light_scale_1_0", || {
        check("Default Light", 1.0, "theme_default_light_1_0")
    });
}

#[test]
fn default_light_scale_1_1() {
    run_on_lavapipe("default_light_scale_1_1", || {
        check("Default Light", 1.1, "theme_default_light_1_1")
    });
}
