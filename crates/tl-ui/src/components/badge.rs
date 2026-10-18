//! `TlBadge`: status pills and count badges (UI-FND-027, SYS-003, SYS-020, SYS-029).
//!
//! A pill 18 px high, padding x `sp3`, fully rounded, `t_xs` text at weight 500, at least 18 px
//! and at most 120 px wide. Longer text ends with "…". All sizes are × ui_scale.
//!
//! Status kinds fill with their status colour at 0.2 alpha and draw the text in the full status
//! colour. The fill is translucent, so on a selected or hovered table row the row colour shows
//! through it and the text keeps its status colour. `.dot(true)` adds a leading 6 px dot in the
//! text colour, so a service state does not rely on colour alone.
//!
//! Owners place the badge in a cell with `items_center` and the cell's left padding. The badge
//! has no interaction; any tooltip comes from the owner.
//!
//! Built from a plain `div`: the kit `Badge` is a count overlay and the kit `Tag` has another
//! height.

use gpui_kit::component::h_flex;
use gpui_kit::{
    App, Div, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window, div, px,
};

use crate::text::style::{TNUM, label_style};
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Pill height and minimum width before the UI scale.
pub const HEIGHT: f32 = 18.0;
/// Widest pill before the UI scale. Longer text is end-ellipsized.
pub const MAX_WIDTH: f32 = 120.0;
/// Leading dot diameter before the UI scale.
pub const DOT: f32 = 6.0;
/// Alpha of the status fill.
pub const STATUS_FILL_ALPHA: f32 = 0.2;
/// Alpha of the `Count` fill.
pub const COUNT_FILL_ALPHA: f32 = 0.25;
/// The largest count shown as a number. Larger counts show `99+`.
pub const COUNT_CAP: u32 = 99;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlBadgeKind {
    #[default]
    Neutral,
    Success,
    Warning,
    Danger,
    Info,
    Count,
}

/// Fill and text colour of one kind.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeColors {
    pub bg: Hsla,
    pub text: Hsla,
}

/// The colours of `kind` in `theme`.
pub fn badge_colors(kind: TlBadgeKind, theme: &TlTheme) -> BadgeColors {
    let status = |c: Hsla| BadgeColors {
        bg: c.alpha(STATUS_FILL_ALPHA),
        text: c,
    };
    match kind {
        TlBadgeKind::Neutral => BadgeColors {
            bg: theme.header,
            text: theme.text_primary,
        },
        TlBadgeKind::Success => status(theme.status.success),
        TlBadgeKind::Warning => status(theme.status.warning),
        TlBadgeKind::Danger => status(theme.status.danger),
        TlBadgeKind::Info => status(theme.status.info),
        TlBadgeKind::Count => BadgeColors {
            bg: theme.accent.alpha(COUNT_FILL_ALPHA),
            text: theme.text_primary,
        },
    }
}

/// The text of a count badge: the number up to 99, `99+` above.
pub fn count_text(count: u32) -> SharedString {
    if count > COUNT_CAP {
        SharedString::new_static("99+")
    } else {
        count.to_string().into()
    }
}

#[derive(IntoElement)]
pub struct TlBadge {
    kind: TlBadgeKind,
    text: SharedString,
    dot: bool,
    max_width: f32,
}

impl TlBadge {
    pub fn new(kind: TlBadgeKind, text: SharedString) -> Self {
        Self {
            kind,
            text,
            dot: false,
            max_width: MAX_WIDTH,
        }
    }

    /// Raises the width limit (× ui_scale) for a status text the spec fixes, such as
    /// "Enabled (existing rule)". The default is [`MAX_WIDTH`].
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = width;
        self
    }

    /// Draws a leading 6 px dot in the text colour. Used for service states.
    pub fn dot(mut self, dot: bool) -> Self {
        self.dot = dot;
        self
    }

    /// Sets the text of a `Count` badge to `count`, or `99+` above 99.
    pub fn count(mut self, count: u32) -> Self {
        self.text = count_text(count);
        self
    }

    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

impl RenderOnce for TlBadge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let colors = badge_colors(self.kind, current(cx));
        let s = m.ui_scale;

        let text = text_box(self.kind, &m)
            .text_color(colors.text)
            .child(self.text);

        let dot = self.dot.then(|| {
            div()
                .debug_selector(|| "TlBadge-dot".into())
                .flex_none()
                .size(px(DOT * s))
                .rounded_full()
                .bg(colors.text)
        });

        h_flex()
            .debug_selector(|| "TlBadge".into())
            .flex_none()
            .justify_center()
            .gap(m.sp[2])
            .h(px(HEIGHT * s))
            .min_w(px(HEIGHT * s))
            .max_w(px(self.max_width * s))
            .px(m.sp[3])
            .rounded_full()
            .bg(colors.bg)
            .overflow_hidden()
            .children(dot)
            .child(text)
    }
}

/// The text part: `t_xs` at weight 500 (`tnum` for counts), one line, end ellipsis.
fn text_box(kind: TlBadgeKind, m: &UiMetrics) -> Div {
    let mut text = div().debug_selector(|| "TlBadge-text".into()).min_w_0();
    *text.text_style() = label_style(m, m.text.xs, FontWeight::MEDIUM);
    if kind == TlBadgeKind::Count {
        text = text.font_features(TNUM.clone());
    }
    text.truncate()
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, Entity, Render, TestAppContext, TextOverflow, VisualTestContext, WhiteSpace,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    const KINDS: [TlBadgeKind; 6] = [
        TlBadgeKind::Neutral,
        TlBadgeKind::Success,
        TlBadgeKind::Warning,
        TlBadgeKind::Danger,
        TlBadgeKind::Info,
        TlBadgeKind::Count,
    ];

    struct Host {
        badge: Box<dyn Fn() -> TlBadge>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(400.)).child((self.badge)())
        }
    }

    fn open(
        cx: &mut TestAppContext,
        badge: impl Fn() -> TlBadge + 'static,
    ) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::store::init(
                cx,
                &ThemeSettings::default(),
                &AppearanceSettings::default(),
                Vec::new(),
            );
        });
        let (view, cx) = cx.add_window_view(|_, _| Host {
            badge: Box::new(badge),
        });
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui_kit::test]
    fn kinds_render(cx: &mut TestAppContext) {
        let (view, cx) = open(cx, || TlBadge::new(TlBadgeKind::Neutral, "x".into()));
        let theme = cx.update(|_, cx| current(cx).clone());

        let c = |kind| badge_colors(kind, &theme);
        assert_eq!(
            c(TlBadgeKind::Neutral),
            BadgeColors {
                bg: theme.header,
                text: theme.text_primary
            }
        );
        for (kind, status) in [
            (TlBadgeKind::Success, theme.status.success),
            (TlBadgeKind::Warning, theme.status.warning),
            (TlBadgeKind::Danger, theme.status.danger),
            (TlBadgeKind::Info, theme.status.info),
        ] {
            let got = c(kind);
            assert_eq!(got.text, status, "{kind:?} text");
            assert_eq!(got.bg, status.alpha(0.2), "{kind:?} fill");
            assert_ne!(got.bg, got.text, "{kind:?} fill is translucent");
        }
        assert_eq!(
            c(TlBadgeKind::Count),
            BadgeColors {
                bg: theme.accent.alpha(0.25),
                text: theme.text_primary
            }
        );

        // Every kind draws an 18 px pill, at least 18 px wide.
        for kind in KINDS {
            view.update(cx, |host, cx| {
                host.badge = Box::new(move || TlBadge::new(kind, "x".into()));
                cx.notify();
            });
            cx.run_until_parked();
            let pill = cx.debug_bounds("TlBadge").expect("badge drawn");
            assert_eq!(pill.size.height, px(18.), "{kind:?}");
            assert!(pill.size.width >= px(18.), "{kind:?}");
        }
    }

    #[test]
    fn count_caps_at_99_plus() {
        let text = |n| TlBadge::new(TlBadgeKind::Count, SharedString::default()).count(n);
        assert_eq!(text(150).text().as_ref(), "99+");
        assert_eq!(text(42).text().as_ref(), "42");
        assert_eq!(text(99).text().as_ref(), "99");
        assert_eq!(text(100).text().as_ref(), "99+");
    }

    #[gpui_kit::test]
    fn dot_drawn(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, || {
            TlBadge::new(TlBadgeKind::Success, "running".into()).dot(true)
        });
        let dot = cx.debug_bounds("TlBadge-dot").expect("dot drawn");
        let text = cx.debug_bounds("TlBadge-text").expect("text drawn");
        assert_eq!(dot.size.width, px(6.));
        assert_eq!(dot.size.height, px(6.));
        assert!(dot.right() <= text.left(), "the dot precedes the text");
        let theme = cx.update(|_, cx| current(cx).clone());
        assert_eq!(
            badge_colors(TlBadgeKind::Success, &theme).text,
            theme.status.success,
            "the dot uses the text colour"
        );
    }

    #[gpui_kit::test]
    fn max_width_can_be_raised(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, || {
            TlBadge::new(TlBadgeKind::Success, "Enabled (existing rule)".into()).max_width(180.)
        });
        let pill = cx.debug_bounds("TlBadge").expect("badge drawn");
        let text = cx.debug_bounds("TlBadge-text").expect("text drawn");
        assert!(pill.size.width > px(120.), "wider than the default limit");
        assert!(pill.size.width <= px(180.));
        assert!(text.right() <= pill.right());
    }

    #[gpui_kit::test]
    fn no_dot_by_default(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, || TlBadge::new(TlBadgeKind::Success, "running".into()));
        assert!(cx.debug_bounds("TlBadge-dot").is_none());
    }

    #[gpui_kit::test]
    fn max_width_ellipsis(cx: &mut TestAppContext) {
        let long: SharedString = "activating (auto-restart) after a failed start, waiting".into();
        let (_, cx) = open(cx, move || TlBadge::new(TlBadgeKind::Warning, long.clone()));
        let pill = cx.debug_bounds("TlBadge").expect("badge drawn");
        assert_eq!(pill.size.width, px(120.));
        let text = cx.debug_bounds("TlBadge-text").expect("text drawn");
        assert!(
            text.right() <= pill.right(),
            "the text stays inside the pill"
        );

        let m = cx.update(|_, cx| *metrics(cx));
        let mut text = text_box(TlBadgeKind::Warning, &m);
        let style = text.text_style();
        assert_eq!(style.white_space, Some(WhiteSpace::Nowrap));
        assert_eq!(
            style.text_overflow,
            Some(TextOverflow::Truncate("…".into())),
            "ends with an ellipsis"
        );
        assert_eq!(style.font_weight, Some(FontWeight::MEDIUM));
        assert_eq!(style.font_size, Some(px(11.).into()));
        assert!(style.font_features.is_none(), "labels have no tnum");
        let mut count = text_box(TlBadgeKind::Count, &m);
        assert!(
            count.text_style().font_features.is_some(),
            "counts use tnum"
        );
    }
}
