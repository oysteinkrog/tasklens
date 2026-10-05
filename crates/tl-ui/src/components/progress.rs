//! `TlProgress` and `TlSpinner`: a progress bar and a loading spinner (UI-FND-023).
//!
//! Allowed uses: toasts, dialog footers, the Properties General tab hash row, the setup window,
//! the Strings tab header and the Services status text. Never in a table cell, a chart, a
//! sidebar card or the main window's status bar (ADR-009).
//!
//! Budget: at most 2 spinners or indeterminate bars visible per window. While present they
//! request a frame every frame, so owners remove them from the tree as soon as the work ends.
//! Never hide them with `visibility: hidden`.
//!
//! Reduced motion is `cx.reduce_motion() || !settings.motion.animations`. The owner passes it in
//! with `reduced_motion`; `cx.reduce_motion()` alone also counts. Under reduced motion the
//! indeterminate bar is a static 30% segment at the left (the owner adds the text "Working…")
//! and the spinner is a static icon at 50% opacity. Neither requests frames then.
//!
//! The bar is drawn on the unstyled kit base parts, not the styled kit `Progress`: the styled one
//! draws its track in the fill colour at 20%, slides a growing segment over 1 s and centres its
//! reduced-motion segment, which all differ from UI-FND-023. A determinate value never animates.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use gpui_kit::base::{Progress as BaseProgress, ProgressIndicator, ProgressTrack};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable, Size, h_flex};
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, Div, ElementId, FontFeatures, Hsla, IntoElement,
    ParentElement, Pixels, RenderOnce, Styled, Window, div, px, relative,
};

use crate::components::icons::{TlIcon, icon};
use crate::theme::{current, metrics};

/// Width of the indeterminate segment, as a fraction of the track.
pub const SEGMENT: f32 = 0.3;
/// One pass of the indeterminate segment from left to right.
pub const SWEEP: Duration = Duration::from_millis(1200);
/// Spinner opacity under reduced motion.
pub const STATIC_SPINNER_OPACITY: f32 = 0.5;

/// Tabular figures for the percent label. `text/style.rs::TNUM` replaces this when it lands.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// Bar height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlProgressSize {
    /// 4 px at 100% scale.
    Sm,
    /// 6 px at 100% scale.
    #[default]
    Md,
}

impl TlProgressSize {
    fn height(self) -> f32 {
        match self {
            Self::Sm => 4.,
            Self::Md => 6.,
        }
    }
}

/// The 1% bucket of a value: `value` clamped to `0..=100`, rounded down.
pub fn bucket(value: f32) -> u32 {
    value.clamp(0., 100.) as u32
}

/// A progress bar, as wide as its parent. `value(None)` (the default) is indeterminate.
#[derive(IntoElement)]
pub struct TlProgress {
    value: Option<f32>,
    size: TlProgressSize,
    show_label: bool,
    reduced_motion: bool,
}

impl Default for TlProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl TlProgress {
    pub fn new() -> Self {
        Self {
            value: None,
            size: TlProgressSize::Md,
            show_label: false,
            reduced_motion: false,
        }
    }

    /// A determinate bar at `value` percent (0 to 100). The bar draws the 1% bucket.
    pub fn value(mut self, value: impl Into<Option<f32>>) -> Self {
        self.value = value.into();
        self
    }

    pub fn size(mut self, size: TlProgressSize) -> Self {
        self.size = size;
        self
    }

    /// Shows a right-aligned percent label ("37%") after a determinate bar.
    pub fn show_label(mut self, show: bool) -> Self {
        self.show_label = show;
        self
    }

    /// `!settings.motion.animations` from the owner.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }

    /// The bar's element id. It holds the 1% bucket, so a value change inside one bucket keeps
    /// the id and a change to another bucket gives a fresh element with no transition.
    pub fn element_id(&self) -> ElementId {
        match self.value {
            Some(v) => ElementId::NamedInteger("tl-progress".into(), u64::from(bucket(v))),
            None => ElementId::Name("tl-progress-indeterminate".into()),
        }
    }
}

impl RenderOnce for TlProgress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let reduced = self.reduced_motion || cx.reduce_motion();
        let id = self.element_id();

        let fill = ProgressIndicator::new()
            .absolute()
            .top_0()
            .h_full()
            .rounded_full()
            .bg(theme.accent);
        let indicator: AnyElement = match self.value {
            Some(v) => fill
                .left_0()
                .w(relative(bucket(v) as f32 / 100.))
                .into_any_element(),
            None if reduced => static_segment(fill).into_any_element(),
            None => fill
                .w(relative(SEGMENT))
                .with_animation(
                    "tl-progress-sweep",
                    Animation::new(SWEEP).repeat(),
                    |this, delta| this.left(relative(-SEGMENT + (1. + SEGMENT) * delta)),
                )
                .into_any_element(),
        };

        let bar = BaseProgress::new(id)
            .value(self.value.map_or(0., |v| bucket(v) as f32))
            .indeterminate(self.value.is_none())
            .relative()
            .flex_1()
            .h(px(self.size.height() * m.ui_scale))
            .rounded_full()
            .overflow_hidden()
            .child(
                ProgressTrack::new()
                    .absolute()
                    .size_full()
                    .rounded_full()
                    .bg(theme.scroll_track),
            )
            .child(indicator);

        let label = match self.value {
            Some(v) if self.show_label => Some(
                div()
                    .flex_none()
                    .text_size(m.text.xs)
                    .font_features(TNUM.clone())
                    .text_color(theme.text_secondary)
                    .child(format!("{}%", bucket(v))),
            ),
            _ => None,
        };

        h_flex().w_full().gap(m.sp[3]).child(bar).children(label)
    }
}

/// The reduced-motion stand-in for the sweep: a still 30% segment at the left.
fn static_segment(fill: ProgressIndicator) -> ProgressIndicator {
    fill.left_0().w(relative(SEGMENT))
}

/// A rotating Lucide `loader-circle`.
#[derive(IntoElement)]
pub struct TlSpinner {
    size: Pixels,
    color: Option<Hsla>,
    reduced_motion: bool,
}

impl TlSpinner {
    /// `size` is `UiMetrics.icon.sm` or `UiMetrics.icon.md`.
    pub fn new(size: Pixels) -> Self {
        Self {
            size,
            color: None,
            reduced_motion: false,
        }
    }

    /// Overrides the default colour, `text_secondary`.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    /// `!settings.motion.animations` from the owner.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }
}

impl RenderOnce for TlSpinner {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(current(cx).text_secondary);
        if self.reduced_motion || cx.reduce_motion() {
            static_spinner(self.size, color).into_any_element()
        } else {
            // The kit spinner turns 360° per 0.8 s.
            Spinner::new()
                .icon(Icon::new(TlIcon::LoaderCircle))
                .with_size(Size::Size(self.size))
                .color(color)
                .into_any_element()
        }
    }
}

fn static_spinner(size: Pixels, color: Hsla) -> Div {
    div()
        .flex_none()
        .opacity(STATIC_SPINNER_OPACITY)
        .child(icon(TlIcon::LoaderCircle, size, color))
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::v_flex;
    use gpui_kit::{Context, DefiniteLength, Length, Render, TestAppContext, size};
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    type Build = Box<dyn Fn() -> AnyElement>;

    /// Renders `build()` while `show` is true.
    struct Host {
        build: Build,
        show: bool,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut root = v_flex().w(px(300.));
            if self.show {
                root = root.child((self.build)());
            }
            root
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

    fn open(
        cx: &mut TestAppContext,
        build: impl Fn() -> AnyElement + 'static,
    ) -> gpui_kit::WindowHandle<Host> {
        let window = cx.open_window(size(px(400.), px(200.)), move |_, _| Host {
            build: Box::new(build),
            show: true,
        });
        cx.run_until_parked();
        window
    }

    fn frames(cx: &mut TestAppContext, window: gpui_kit::WindowHandle<Host>) -> usize {
        window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap()
    }

    #[gpui_kit::test]
    fn value_bucket_id(cx: &mut TestAppContext) {
        let id = |v: f32| TlProgress::new().value(v).element_id();
        assert_eq!(id(37.4), id(37.9), "same bucket 37");
        assert_eq!(id(37.4), ElementId::NamedInteger("tl-progress".into(), 37));
        assert_ne!(id(38.0), id(37.9), "bucket 38");
        assert_eq!(bucket(-5.), 0);
        assert_eq!(bucket(140.), 100);
        assert_ne!(TlProgress::new().element_id(), id(0.));

        setup(cx);
        let window = open(cx, || {
            TlProgress::new()
                .value(37.4)
                .show_label(true)
                .into_any_element()
        });
        assert_eq!(frames(cx, window), 0, "a determinate bar requests no frame");
        window
            .update(cx, |host, _, cx| {
                host.build = Box::new(|| TlProgress::new().value(37.9).into_any_element());
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(frames(cx, window), 0, "a value change inside a bucket");
        window
            .update(cx, |host, _, cx| {
                host.build = Box::new(|| TlProgress::new().value(38.0).into_any_element());
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            frames(cx, window),
            0,
            "a value change to the next bucket jumps"
        );
    }

    #[gpui_kit::test]
    fn reduced_motion_static(cx: &mut TestAppContext) {
        setup(cx);

        // The owner's flag alone.
        let window = open(cx, || {
            v_flex()
                .child(TlProgress::new().reduced_motion(true))
                .child(TlSpinner::new(px(16.)).reduced_motion(true))
                .into_any_element()
        });
        assert_eq!(frames(cx, window), 0, "owner flag: no frame");

        // The system setting alone.
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = open(cx, || {
            v_flex()
                .child(TlProgress::new())
                .child(TlSpinner::new(px(16.)))
                .into_any_element()
        });
        assert_eq!(frames(cx, window), 0, "system setting: no frame");

        let mut segment = static_segment(ProgressIndicator::new());
        let style = segment.style();
        assert_eq!(
            style.size.width,
            Some(Length::Definite(DefiniteLength::Fraction(SEGMENT)))
        );
        assert_eq!(SEGMENT, 0.3);
        assert_eq!(style.inset.left, Some(px(0.).into()));

        let mut spinner = static_spinner(px(16.), gpui_kit::black());
        assert_eq!(spinner.style().opacity, Some(STATIC_SPINNER_OPACITY));
        assert_eq!(STATIC_SPINNER_OPACITY, 0.5);
    }

    #[gpui_kit::test]
    fn animates_without_reduced_motion(cx: &mut TestAppContext) {
        setup(cx);
        let window = open(cx, || TlProgress::new().into_any_element());
        assert!(frames(cx, window) > 0, "the sweep requests frames");
        let window = open(cx, || TlSpinner::new(px(16.)).into_any_element());
        assert!(frames(cx, window) > 0, "the spinner requests frames");
    }

    #[gpui_kit::test]
    fn removed_when_done(cx: &mut TestAppContext) {
        setup(cx);
        let window = open(cx, || {
            v_flex()
                .child(TlSpinner::new(px(16.)))
                .child(TlProgress::new())
                .into_any_element()
        });
        assert!(frames(cx, window) > 0, "present: frames requested");
        window
            .update(cx, |host, _, cx| {
                host.show = false;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        // The next frame still delivers the requests made by the last frame that drew them.
        assert!(
            frames(cx, window) <= 2,
            "at most one request per animated element"
        );
        cx.run_until_parked();
        assert_eq!(frames(cx, window), 0, "removed: no further frames");
        cx.run_until_parked();
        assert_eq!(frames(cx, window), 0);
    }
}
