//! `TlBanner`: an information or warning strip inside a pane (UI-FND-031, SYS-015, UI-SYS-038,
//! PERF-105, UI-PRC-036, APP-058).
//!
//! A row 32 px high with a status fill at 0.12 alpha and a 1 px bottom border in the same status
//! colour. It holds a 14 px icon, one line of `t_sm` text that ends with "…" when it does not fit,
//! and zero or more `TlButton::Secondary` sm buttons on the right. The full text shows in a hover
//! tooltip. The icon and the text say the kind, so the meaning does not rely on colour alone.
//!
//! A banner has no close button: it states a fact about the data below it. The owner removes it,
//! or adds a "Hide" button. The buttons are ordinary `TlButton`s, so Tab visits them in reading
//! order.

use std::sync::Arc;

use gpui_kit::component::h_flex;
use gpui_kit::{
    Action, App, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};

use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::icons::{TlIcon, icon};
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::t_sm;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Strip height before the UI scale.
pub const HEIGHT: f32 = 32.0;
/// Alpha of the status fill.
pub const FILL_ALPHA: f32 = 0.12;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlBannerKind {
    #[default]
    Info,
    Warning,
}

/// Fill, border, icon and icon colour of one kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerLook {
    pub fill: Hsla,
    pub border: Hsla,
    pub icon: TlIcon,
    pub icon_color: Hsla,
}

/// The look of `kind` in `theme`.
pub fn banner_look(kind: TlBannerKind, theme: &TlTheme) -> BannerLook {
    let (status, icon) = match kind {
        TlBannerKind::Info => (theme.status.info, TlIcon::Info),
        TlBannerKind::Warning => (theme.status.warning, TlIcon::TriangleAlert),
    };
    BannerLook {
        fill: status.alpha(FILL_ALPHA),
        border: status,
        icon,
        icon_color: status,
    }
}

#[derive(IntoElement)]
pub struct TlBanner {
    kind: TlBannerKind,
    text: SharedString,
    buttons: Vec<(SharedString, Box<dyn Action>)>,
}

impl TlBanner {
    pub fn new(kind: TlBannerKind, text: SharedString) -> Self {
        Self {
            kind,
            text,
            buttons: Vec::new(),
        }
    }

    /// Adds a trailing button with `label` that dispatches `action`. Buttons show in the order
    /// they are added.
    pub fn button(mut self, label: SharedString, action: Box<dyn Action>) -> Self {
        self.buttons.push((label, action));
        self
    }
}

impl RenderOnce for TlBanner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let look = banner_look(self.kind, &theme);

        // The tooltip state is keyed by the text, so a banner that changes its text starts fresh.
        let tooltip_id = ElementId::NamedChild(
            Arc::new(ElementId::Name(self.text.clone())),
            "TlBanner-tooltip".into(),
        );
        let text = text_box(&m)
            .relative()
            .flex_1()
            .text_color(theme.text_primary)
            .child(self.text.clone());
        let state = HoverTooltip::state(tooltip_id, window, cx);
        let text = HoverTooltip::attach(
            &state,
            text,
            DELAY_CONTROL,
            TlTooltip::text(self.text).build(),
            window,
            cx,
        );

        let buttons = (!self.buttons.is_empty()).then(|| {
            h_flex()
                .flex_none()
                .gap(m.sp[4])
                .children(self.buttons.into_iter().map(|(label, action)| {
                    TlButton::new(
                        ElementId::Name(format!("TlBanner-{label}").into()),
                        TlButtonKind::Secondary,
                    )
                    .size(TlButtonSize::Sm)
                    .label(label)
                    .action(action)
                }))
        });

        h_flex()
            .debug_selector(|| "TlBanner".into())
            .w_full()
            .h(px(HEIGHT * m.ui_scale))
            .px(m.sp[5])
            .gap(m.sp[3])
            .bg(look.fill)
            .border_b_1()
            .border_color(look.border)
            .child(
                div()
                    .flex_none()
                    .child(icon(look.icon, m.icon.sm, look.icon_color)),
            )
            .child(text)
            .children(buttons)
    }
}

/// The text part: `t_sm`, one line, end ellipsis.
fn text_box(m: &UiMetrics) -> Div {
    let mut text = div().debug_selector(|| "TlBanner-text".into()).min_w_0();
    *text.text_style() = t_sm(m);
    text.truncate()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui_kit::{
        Background, Context, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render,
        TestAppContext, TestDispatcher, TextOverflow, TextRun, VisualTestContext, WhiteSpace,
        black, font,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::text::fonts::{UI_FAMILY, load_bundled_fonts};

    gpui_kit::actions!(tl_banner_test, [First, Second]);

    const WIDTH: f32 = 400.;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// A test context that shapes text with the real system text stack and the bundled fonts,
    /// so text widths are real.
    fn text_cx() -> TestAppContext {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
            setup(cx);
        });
        cx
    }

    /// The banner in a 400 px column, recording which test actions reach the root.
    struct Host {
        banner: Box<dyn Fn() -> TlBanner>,
        hits: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let first = self.hits.clone();
            let second = self.hits.clone();
            div()
                .id("root")
                .tab_group()
                .w(px(WIDTH))
                .h(px(300.))
                .on_action(move |_: &First, _, _| first.borrow_mut().push("first"))
                .on_action(move |_: &Second, _, _| second.borrow_mut().push("second"))
                .child((self.banner)())
        }
    }

    fn open(
        cx: &mut TestAppContext,
        banner: impl Fn() -> TlBanner + 'static,
    ) -> (&mut VisualTestContext, Rc<RefCell<Vec<&'static str>>>) {
        let hits = Rc::new(RefCell::new(Vec::new()));
        let (_, cx) = cx.add_window_view({
            let hits = hits.clone();
            move |_, _| Host {
                banner: Box::new(banner),
                hits,
            }
        });
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, hits)
    }

    /// Counts painted quads with the `fill` background, and quads with a 1 px bottom border in
    /// `border`.
    fn quads(cx: &mut VisualTestContext, fill: Hsla, border: Hsla) -> (usize, usize) {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let one = window.scale_factor();
            let quads = window.painted_quads();
            let fills = quads
                .iter()
                .filter(|q| q.background == Background::from(fill))
                .count();
            let borders = quads
                .iter()
                .filter(|q| q.border_color == border && q.border_widths.bottom.0 == one)
                .count();
            (fills, borders)
        })
    }

    #[gpui_kit::test]
    fn info_and_warning_colours(cx: &mut TestAppContext) {
        cx.update(setup);
        let theme = cx.update(|cx| current(cx).clone());

        let info = banner_look(TlBannerKind::Info, &theme);
        assert_eq!(info.fill, theme.status.info.alpha(0.12));
        assert_eq!(info.border, theme.status.info);
        assert_eq!(info.icon, TlIcon::Info);
        assert_eq!(info.icon_color, theme.status.info);

        let warning = banner_look(TlBannerKind::Warning, &theme);
        assert_eq!(warning.fill, theme.status.warning.alpha(0.12));
        assert_eq!(warning.border, theme.status.warning);
        assert_eq!(warning.icon, TlIcon::TriangleAlert);
        assert_eq!(warning.icon_color, theme.status.warning);

        let (cx, _) = open(cx, || {
            TlBanner::new(TlBannerKind::Warning, "Recording is off.".into())
        });
        let strip = cx.debug_bounds("TlBanner").expect("banner drawn");
        assert_eq!(strip.size.height, px(32.));
        assert_eq!(strip.size.width, px(WIDTH));
        let (fills, borders) = quads(cx, warning.fill, warning.border);
        assert_eq!(fills, 1, "warning fill at 0.12");
        assert!(borders > 0, "1 px warning bottom border");
        assert_eq!(
            quads(cx, info.fill, info.border),
            (0, 0),
            "a warning banner paints no info colour"
        );
    }

    #[test]
    fn long_text_ellipsis_tooltip() {
        let mut cx = text_cx();
        let long: SharedString = "The system journal cannot be read. Add your user to the \
             systemd-journal group, or run tasklens with access to /var/log/journal, then \
             open this tab again."
            .into();
        let (cx, _) = open(&mut cx, {
            let long = long.clone();
            move || {
                TlBanner::new(TlBannerKind::Info, long.clone())
                    .button("Additional setup…".into(), Box::new(First))
            }
        });

        let strip = cx.debug_bounds("TlBanner").expect("banner drawn");
        let text = cx.debug_bounds("TlBanner-text").expect("text drawn");
        let button = cx
            .debug_bounds("TlButton-TlBanner-Additional setup…")
            .expect("button drawn");
        assert!(
            text.right() <= button.left(),
            "the text stops at the button"
        );
        assert!(button.right() <= strip.right(), "the button is inside");

        let (m, full) = cx.update(|window, cx| {
            let m = *metrics(cx);
            let run = TextRun {
                len: long.len(),
                font: font(UI_FAMILY),
                color: black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window
                .text_system()
                .shape_line(long.clone(), m.text.sm, &[run], None);
            (m, line.width)
        });
        assert!(full > text.size.width, "the text is wider than its box");
        let line_height = px(f32::from(m.text.sm) * m.line_height);
        assert_eq!(text.size.height, line_height, "one line");

        // The text box truncates at the end with U+2026.
        let mut style_box = text_box(&m);
        let style = style_box.text_style();
        assert_eq!(style.white_space, Some(WhiteSpace::Nowrap));
        assert_eq!(
            style.text_overflow,
            Some(TextOverflow::Truncate("…".into()))
        );
        assert_eq!(style.font_size, Some(m.text.sm.into()));

        // Hovering the text shows the whole text, wrapped over more than one line.
        cx.simulate_mouse_move(text.center(), None, Modifiers::none());
        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tip = cx
            .debug_bounds("TlTooltip-text")
            .expect("tooltip shown on hover");
        assert!(
            tip.size.height >= line_height * 2.,
            "the tooltip holds the full text: {:?}",
            tip.size
        );
        assert!(
            f32::from(tip.size.width) * (f32::from(tip.size.height) / f32::from(line_height))
                >= f32::from(full) * 0.9,
            "the tooltip area fits the full text"
        );
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        cx.run_until_parked();
    }

    fn focus_next(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.focus_next(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui_kit::test]
    fn buttons_tab_order(cx: &mut TestAppContext) {
        cx.update(setup);
        let (cx, hits) = open(cx, || {
            TlBanner::new(TlBannerKind::Info, "History recording is off.".into())
                .button("Turn on".into(), Box::new(First))
                .button("Hide".into(), Box::new(Second))
        });
        let first = cx
            .debug_bounds("TlButton-TlBanner-Turn on")
            .expect("first button drawn");
        let second = cx
            .debug_bounds("TlButton-TlBanner-Hide")
            .expect("second button drawn");
        assert!(first.right() <= second.left(), "buttons in reading order");
        assert_eq!(first.size.height, px(24.), "sm buttons");

        focus_next(cx);
        press(cx, "enter");
        focus_next(cx);
        press(cx, "enter");
        assert_eq!(*hits.borrow(), ["first", "second"]);
    }
}
