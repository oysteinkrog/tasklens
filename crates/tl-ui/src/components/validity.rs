//! `TlValidityGlyph`: the value or a validity glyph in every numeric cell, header total and
//! sidebar readout (UI-FND-026, ADR-003, R-050, PERF-013, PROC-021).
//!
//! | Validity | Shows | Colour | Tooltip |
//! |---|---|---|---|
//! | Valid | the value | `text_primary` | none |
//! | WarmingUp | `…` | `text_secondary` | "Collecting first sample" |
//! | Stale | the last value | `text_secondary` | "Last updated {age} ago" |
//! | Restricted | Lucide `lock`, 12 px | `text_secondary` | "Needs administrator access." |
//! | Unsupported | `n/a` | `text_secondary` | "Not available on this device" |
//! | Missing | `-` | `text_secondary` | "No longer present" |
//! | Failed | `!` | `status.warning` | "Could not read: {last_error}" |
//!
//! On a selected row `text_primary` becomes `text_selected_primary` and `text_secondary` becomes
//! `text_selected_secondary`. The glyph fills the cell width and sits at the right edge, like a
//! number, so a change from `…` to a value moves nothing. Heat tint is for Valid cells only
//! ([`glyph_allows_heat`]). Charts draw validity themselves (E19).
//!
//! The tooltip text is built only when the tooltip opens, from the [`GlyphCtx`] the owner passes
//! to [`glyph_with_tooltip`].

use std::sync::Arc;

use gpui_kit::component::h_flex;
use gpui_kit::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, Window, div,
};
use tl_model::units::short_age;
use tl_model::validity::Validity;

use super::icons::{TlIcon, icon};
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::text::style::numeric_style;
use crate::theme::{TlTheme, UiMetrics};

static WARMING: SharedString = SharedString::new_static("…");
static UNSUPPORTED: SharedString = SharedString::new_static("n/a");
static MISSING: SharedString = SharedString::new_static("-");
static FAILED: SharedString = SharedString::new_static("!");
static RESTRICTED_COPY: SharedString = SharedString::new_static("Restricted");

/// Restricted tooltip once the privileged helper exists (P3, E29).
pub const RESTRICTED_TOOLTIP_P3: &str = "Needs administrator access. Click the shield to grant it.";
/// Restricted tooltip before P3.
pub const RESTRICTED_TOOLTIP_PRE_P3: &str = "Needs administrator access.";

const WARMING_TOOLTIP: &str = "Collecting first sample";
const UNSUPPORTED_TOOLTIP: &str = "Not available on this device";
const MISSING_TOOLTIP: &str = "No longer present";

/// The Restricted tooltip. Callers pass `false` until E29 lands.
pub fn restricted_tooltip(helper_available: bool) -> &'static str {
    if helper_available {
        RESTRICTED_TOOLTIP_P3
    } else {
        RESTRICTED_TOOLTIP_PRE_P3
    }
}

/// Heat tint is drawn only behind Valid cells.
pub fn glyph_allows_heat(validity: Validity) -> bool {
    validity.is_valid()
}

/// What the owner knows about the value, read only when the tooltip opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphCtx {
    /// Age of the last fresh sample, for Stale.
    pub age_ns: u64,
    /// The source's last error, for Failed.
    pub last_error: Option<Arc<str>>,
    /// The privileged helper is available (E29), for Restricted.
    pub helper_available: bool,
}

/// What a cell draws.
#[derive(Clone, Debug, PartialEq)]
pub enum GlyphContent {
    Text(SharedString),
    Lock,
}

/// The content and colour of a cell in `validity`.
pub fn glyph_parts(
    validity: Validity,
    value_text: &SharedString,
    selected: bool,
    theme: &TlTheme,
) -> (GlyphContent, Hsla) {
    let primary = if selected {
        theme.text_selected_primary
    } else {
        theme.text_primary
    };
    let secondary = if selected {
        theme.text_selected_secondary
    } else {
        theme.text_secondary
    };
    match validity {
        Validity::Valid => (GlyphContent::Text(value_text.clone()), primary),
        Validity::WarmingUp => (GlyphContent::Text(WARMING.clone()), secondary),
        Validity::Stale => (GlyphContent::Text(value_text.clone()), secondary),
        Validity::Restricted => (GlyphContent::Lock, secondary),
        Validity::Unsupported => (GlyphContent::Text(UNSUPPORTED.clone()), secondary),
        Validity::Missing => (GlyphContent::Text(MISSING.clone()), secondary),
        Validity::Failed => (GlyphContent::Text(FAILED.clone()), theme.status.warning),
    }
}

/// The text a copy action puts on the clipboard (ADR-015 copy formats).
pub fn copy_text(validity: Validity, value_text: &SharedString) -> SharedString {
    match validity {
        Validity::Valid | Validity::Stale => value_text.clone(),
        Validity::WarmingUp => WARMING.clone(),
        Validity::Restricted => RESTRICTED_COPY.clone(),
        Validity::Unsupported => UNSUPPORTED.clone(),
        Validity::Missing => MISSING.clone(),
        Validity::Failed => FAILED.clone(),
    }
}

/// The tooltip text, or `None` for Valid.
pub fn tooltip_text(validity: Validity, ctx: &GlyphCtx) -> Option<SharedString> {
    let text: SharedString = match validity {
        Validity::Valid => return None,
        Validity::WarmingUp => WARMING_TOOLTIP.into(),
        Validity::Stale => {
            let mut age = String::from("Last updated ");
            short_age(&mut age, ctx.age_ns / 1_000_000_000);
            age.push_str(" ago");
            age.into()
        }
        Validity::Restricted => restricted_tooltip(ctx.helper_available).into(),
        Validity::Unsupported => UNSUPPORTED_TOOLTIP.into(),
        Validity::Missing => MISSING_TOOLTIP.into(),
        Validity::Failed => format!(
            "Could not read: {}",
            ctx.last_error.as_deref().unwrap_or("")
        )
        .into(),
    };
    Some(text)
}

/// The cell content without a tooltip: one right-aligned numeric text run, or the lock icon
/// centred vertically. It fills the cell width.
pub fn glyph(
    validity: Validity,
    value_text: &SharedString,
    selected: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) -> AnyElement {
    base(validity, value_text, selected, m, theme).into_any_element()
}

/// [`glyph`] with the validity tooltip after [`DELAY_CONTROL`], through [`HoverTooltip`]. `id`
/// must be unique among the cell's siblings. Valid cells get no tooltip.
#[allow(clippy::too_many_arguments)]
pub fn glyph_with_tooltip(
    id: impl Into<ElementId>,
    validity: Validity,
    value_text: &SharedString,
    selected: bool,
    ctx: &GlyphCtx,
    m: &UiMetrics,
    theme: &TlTheme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let id = id.into();
    let cell = base(validity, value_text, selected, m, theme)
        .relative()
        .id(id.clone());
    if validity.is_valid() {
        return cell.into_any_element();
    }
    let ctx = ctx.clone();
    let build = move |_: &mut Window, cx: &mut App| {
        let text = tooltip_text(validity, &ctx).unwrap_or_default();
        TlTooltip::text(text).view(cx)
    };
    let state = HoverTooltip::state(
        ElementId::NamedChild(Arc::new(id), "tooltip".into()),
        window,
        cx,
    );
    HoverTooltip::attach(&state, cell, DELAY_CONTROL, build, window, cx).into_any_element()
}

fn base(
    validity: Validity,
    value_text: &SharedString,
    selected: bool,
    m: &UiMetrics,
    theme: &TlTheme,
) -> gpui_kit::Div {
    let (content, color) = glyph_parts(validity, value_text, selected, theme);
    let mut cell = h_flex()
        .debug_selector(|| "TlValidityGlyph".into())
        .w_full()
        .justify_end();
    *cell.text_style() = numeric_style(m, m.base_text);
    let inner = match content {
        GlyphContent::Text(text) => div()
            .debug_selector(|| "TlValidityGlyph-text".into())
            .flex_none()
            .whitespace_nowrap()
            .text_color(color)
            .child(text),
        GlyphContent::Lock => div()
            .debug_selector(|| "TlValidityGlyph-lock".into())
            .flex_none()
            .child(icon(TlIcon::Lock, m.icon.xs, color)),
    };
    cell.child(inner)
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        App, Context, Modifiers, MouseMoveEvent, Render, TestAppContext, TestDispatcher, Window,
        point, px,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::text::fonts::load_bundled_fonts;
    use crate::text::metrics::TextMetricsCache;
    use crate::theme::{current, metrics};

    const ALL: [Validity; 7] = [
        Validity::Valid,
        Validity::WarmingUp,
        Validity::Stale,
        Validity::Restricted,
        Validity::Unsupported,
        Validity::Missing,
        Validity::Failed,
    ];

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// A test context backed by the real Linux text system, so text has real widths.
    fn cosmic_cx() -> TestAppContext {
        let text_system = gpui_kit::platform::current_platform(true).text_system();
        let cx = TestAppContext::build_with_text_system(TestDispatcher::new(0), None, text_system);
        cx.update(|cx| {
            let report = load_bundled_fonts(cx);
            assert!(report.ui_ok, "failed faces: {:?}", report.failed);
            setup(cx);
        });
        cx
    }

    fn theme(cx: &mut TestAppContext) -> Arc<TlTheme> {
        cx.update(|cx| {
            setup(cx);
            current(cx).clone()
        })
    }

    struct Host {
        validity: Validity,
        ctx: GlyphCtx,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let m = *metrics(cx);
            let theme = current(cx).clone();
            let value: SharedString = "100.0%".into();
            let cell = glyph_with_tooltip(
                "cell",
                self.validity,
                &value,
                false,
                &self.ctx,
                &m,
                &theme,
                window,
                cx,
            );
            div()
                .size(px(400.))
                .child(div().w(px(120.)).child(cell))
        }
    }

    #[test]
    fn glyph_per_state() {
        let mut cx = cosmic_cx();
        let theme = cx.update(|cx| current(cx).clone());
        let value: SharedString = "12.5%".into();
        let expected = [
            (
                Validity::Valid,
                GlyphContent::Text(value.clone()),
                theme.text_primary,
            ),
            (
                Validity::WarmingUp,
                GlyphContent::Text("…".into()),
                theme.text_secondary,
            ),
            (
                Validity::Stale,
                GlyphContent::Text(value.clone()),
                theme.text_secondary,
            ),
            (
                Validity::Restricted,
                GlyphContent::Lock,
                theme.text_secondary,
            ),
            (
                Validity::Unsupported,
                GlyphContent::Text("n/a".into()),
                theme.text_secondary,
            ),
            (
                Validity::Missing,
                GlyphContent::Text("-".into()),
                theme.text_secondary,
            ),
            (
                Validity::Failed,
                GlyphContent::Text("!".into()),
                theme.status.warning,
            ),
        ];
        for (validity, content, color) in expected {
            assert_eq!(
                glyph_parts(validity, &value, false, &theme),
                (content, color),
                "{validity:?}"
            );
        }
        assert!(glyph_allows_heat(Validity::Valid));
        for validity in &ALL[1..] {
            assert!(!glyph_allows_heat(*validity), "{validity:?}");
        }

        // No glyph is wider than the "100.0%" template, and each sits at the cell's right edge.
        let (view, cx) = cx.add_window_view(|_, _| Host {
            validity: Validity::WarmingUp,
            ctx: GlyphCtx::default(),
        });
        cx.run_until_parked();
        let template = cx.update(|window, cx| {
            let m = *metrics(cx);
            TextMetricsCache::default().template_width("100.0%", m.base_text, window, cx)
        });
        for validity in [
            Validity::WarmingUp,
            Validity::Restricted,
            Validity::Unsupported,
            Validity::Missing,
            Validity::Failed,
        ] {
            view.update(cx, |host, cx| {
                host.validity = validity;
                cx.notify();
            });
            cx.run_until_parked();
            let selector = if validity == Validity::Restricted {
                "TlValidityGlyph-lock"
            } else {
                "TlValidityGlyph-text"
            };
            let inner = cx.debug_bounds(selector).expect("glyph drawn");
            let cell = cx.debug_bounds("TlValidityGlyph").expect("cell drawn");
            assert!(
                inner.size.width <= template,
                "{validity:?}: {:?} > {template:?}",
                inner.size.width
            );
            assert!(inner.size.width > px(0.), "{validity:?} has a width");
            assert_eq!(cell.size.width, px(120.), "{validity:?} fills the cell");
            assert_eq!(inner.right(), cell.right(), "{validity:?} is right-aligned");
            if validity == Validity::Restricted {
                assert_eq!(inner.size.width, px(12.), "the lock is i_xs");
                let mid = |b: gpui_kit::Bounds<gpui_kit::Pixels>| b.top() + b.size.height / 2.;
                assert_eq!(mid(inner), mid(cell), "the lock is centred vertically");
            }
        }
    }

    #[gpui_kit::test]
    fn selected_row_colours(cx: &mut TestAppContext) {
        let theme = theme(cx);
        let value: SharedString = "1".into();
        let color = |v| glyph_parts(v, &value, true, &theme).1;
        assert_eq!(color(Validity::WarmingUp), theme.text_selected_secondary);
        assert_eq!(color(Validity::Valid), theme.text_selected_primary);
        for v in [
            Validity::Stale,
            Validity::Restricted,
            Validity::Unsupported,
            Validity::Missing,
        ] {
            assert_eq!(color(v), theme.text_selected_secondary, "{v:?}");
        }
        assert_eq!(color(Validity::Failed), theme.status.warning);
    }

    #[test]
    fn copy_text_per_state() {
        let value: SharedString = "3.2 MiB".into();
        let expected = [
            (Validity::Valid, "3.2 MiB"),
            (Validity::WarmingUp, "…"),
            (Validity::Stale, "3.2 MiB"),
            (Validity::Restricted, "Restricted"),
            (Validity::Unsupported, "n/a"),
            (Validity::Missing, "-"),
            (Validity::Failed, "!"),
        ];
        for (validity, text) in expected {
            assert_eq!(copy_text(validity, &value).as_ref(), text, "{validity:?}");
        }
    }

    #[gpui_kit::test]
    fn tooltip_texts(cx: &mut TestAppContext) {
        let ctx = GlyphCtx {
            age_ns: (4 * 3600 + 12 * 60 + 7) * 1_000_000_000,
            last_error: Some("EIO".into()),
            helper_available: false,
        };
        let text = |v| tooltip_text(v, &ctx).map(|s| s.to_string());
        assert_eq!(text(Validity::Valid), None);
        assert_eq!(
            text(Validity::WarmingUp).as_deref(),
            Some("Collecting first sample")
        );
        assert_eq!(
            text(Validity::Stale).as_deref(),
            Some("Last updated 4h 12m ago")
        );
        assert_eq!(
            text(Validity::Restricted).as_deref(),
            Some("Needs administrator access.")
        );
        assert_eq!(
            text(Validity::Unsupported).as_deref(),
            Some("Not available on this device")
        );
        assert_eq!(
            text(Validity::Missing).as_deref(),
            Some("No longer present")
        );
        assert_eq!(
            text(Validity::Failed).as_deref(),
            Some("Could not read: EIO")
        );
        let p3 = GlyphCtx {
            helper_available: true,
            ..ctx.clone()
        };
        assert_eq!(
            tooltip_text(Validity::Restricted, &p3).as_deref(),
            Some("Needs administrator access. Click the shield to grant it.")
        );
        for (secs, age) in [
            (5, "5s"),
            (12 * 60 + 5, "12m 5s"),
            (86_400 + 4 * 3600, "1d 4h"),
        ] {
            let stale = GlyphCtx {
                age_ns: secs * 1_000_000_000,
                ..GlyphCtx::default()
            };
            assert_eq!(
                tooltip_text(Validity::Stale, &stale).as_deref(),
                Some(format!("Last updated {age} ago").as_str())
            );
        }

        // Hovering a Failed cell opens the tooltip after 500 ms; a Valid cell has none.
        cx.update(setup);
        let (view, cx) = cx.add_window_view(move |_, _| Host {
            validity: Validity::Failed,
            ctx,
        });
        cx.run_until_parked();
        let hover = |cx: &mut gpui_kit::VisualTestContext, x: f32| {
            cx.simulate_event(MouseMoveEvent {
                position: point(px(x), px(5.)),
                pressed_button: None,
                modifiers: Modifiers::none(),
            });
        };
        hover(cx, 110.);
        cx.executor().advance_clock(DELAY_CONTROL / 2);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("TlTooltip").is_none(), "not before 500 ms");
        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("TlTooltip").is_some(),
            "Failed shows a tooltip"
        );

        hover(cx, 300.);
        cx.run_until_parked();
        view.update(cx, |host, cx| {
            host.validity = Validity::Valid;
            cx.notify();
        });
        hover(cx, 110.);
        cx.executor().advance_clock(DELAY_CONTROL * 2);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("TlTooltip").is_none(),
            "Valid has no tooltip"
        );
    }
}
