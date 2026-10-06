//! `TlTooltip`: tooltip content with the tasklens look, widths, line cut and delays (UI-FND-019,
//! R-111, PROC-039).
//!
//! Built on the kit's unstyled base `Tooltip`, which owns the tooltip role. The styled kit
//! `Tooltip` is not used because it adds a 12 px margin and puts the shortcut on the text row,
//! while tasklens puts it on its own right-aligned line.
//!
//! Two ways to show it:
//! - Hover: pass [`TlTooltip::build`] to GPUI's `.tooltip(..)` and set `.tooltip_show_delay` to
//!   [`DELAY_CONTROL`] or [`DELAY_ROW_NAME`]. GPUI owns hover tooltips: it places them at the
//!   pointer, and closes them when the pointer leaves the trigger, on mouse down and on scroll.
//! - Keyboard focus: [`FocusTooltip::attach`] on a focusable trigger. After keyboard focus rests
//!   on the trigger for the delay, the tooltip shows below the trigger, left-aligned, `sp2` away.
//!   It flips above when there is no room below and stays inside the window. Any key down
//!   closes it (Esc is consumed and focus stays), as do mouse down, scroll, focus loss and window
//!   deactivation. It shows again only after focus leaves and comes back.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::{Align, Placement, Positioner, Tooltip as BaseTooltip};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    Action, AnyView, App, AppContext as _, AsKeystroke as _, Bounds, BoxShadow, Context, ElementId,
    Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseDownEvent,
    ParentElement, Pixels, Render, ScrollWheelEvent, SharedString, Styled, Task, Window, canvas,
    deferred, div, point, prelude::FluentBuilder as _, px,
};

use crate::text::style::{t_sm, t_xs};
use crate::theme::{current, metrics};

/// Hover delay for buttons and glyphs (the kit default).
pub const DELAY_CONTROL: Duration = Duration::from_millis(500);
/// Hover delay for table Name cells (PROC-039).
pub const DELAY_ROW_NAME: Duration = Duration::from_millis(600);

/// Max width of [`TlTooltip::text`], × ui_scale.
const TEXT_MAX_WIDTH: f32 = 360.;
/// Max width of [`TlTooltip::lines`], × ui_scale (R-111).
const LINES_MAX_WIDTH: f32 = 560.;
/// More lines than this are cut; the last visible line ends with [`ELLIPSIS`].
pub const MAX_LINES: usize = 12;
/// U+2026.
const ELLIPSIS: &str = "…";
/// Paint order of a keyboard tooltip, above popups (the kit uses 200 for its own tooltips).
const PRIORITY: usize = 200;

/// `"Error: text"`: the visible kind prefix for toasts (UI-FND-020), so the kind is not carried
/// by colour alone.
pub fn with_kind_prefix(kind: &str, text: &str) -> SharedString {
    format!("{kind}: {text}").into()
}

#[derive(Clone)]
enum Body {
    Text(SharedString),
    Lines(Vec<SharedString>),
}

/// Tooltip content. Build it with [`TlTooltip::text`] or [`TlTooltip::lines`], then hand
/// [`TlTooltip::build`] to `.tooltip(..)`, or call [`TlTooltip::view`] inside an owner closure
/// that formats the text when the tooltip opens.
#[derive(Clone)]
pub struct TlTooltip {
    body: Body,
    action: Option<(Rc<dyn Action>, Option<FocusHandle>)>,
}

impl TlTooltip {
    /// One short text. Wraps at 360 × ui_scale.
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            body: Body::Text(text.into()),
            action: None,
        }
    }

    /// Several lines, such as the process row tooltip. Each line starts on its own row and wraps
    /// on its own at 560 × ui_scale.
    pub fn lines(lines: Vec<SharedString>) -> Self {
        Self {
            body: Body::Lines(lines),
            action: None,
        }
    }

    /// Appends a right-aligned line with the key binding of `action`, looked up from `focus` when
    /// given, else from any context. No line when the action has no binding.
    pub fn action(mut self, action: &dyn Action, focus: Option<&FocusHandle>) -> Self {
        self.action = Some((Rc::from(action.boxed_clone()), focus.cloned()));
        self
    }

    /// The tooltip as a view, for an owner closure passed to `.tooltip(..)`.
    pub fn view(self, cx: &mut App) -> AnyView {
        cx.new(|_| self).into()
    }

    /// The closure for GPUI's `.tooltip(..)`. It creates the view when the tooltip opens.
    pub fn build(self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        move |_, cx| self.clone().view(cx)
    }

    fn max_width(&self) -> f32 {
        match self.body {
            Body::Text(_) => TEXT_MAX_WIDTH,
            Body::Lines(_) => LINES_MAX_WIDTH,
        }
    }

    /// The text to lay out: the lines joined by newlines, cut to [`MAX_LINES`].
    fn body_text(&self) -> SharedString {
        match &self.body {
            Body::Text(text) => text.clone(),
            Body::Lines(lines) => cut_lines(lines).join("\n").into(),
        }
    }
}

/// Keeps the first [`MAX_LINES`] lines. When lines were dropped, the last kept line ends with
/// "…". Wrapped lines that still run past [`MAX_LINES`] rows are cut at layout time.
fn cut_lines(lines: &[SharedString]) -> Vec<SharedString> {
    if lines.len() <= MAX_LINES {
        return lines.to_vec();
    }
    let mut kept = lines[..MAX_LINES].to_vec();
    let last = &kept[MAX_LINES - 1];
    kept[MAX_LINES - 1] = format!("{}{ELLIPSIS}", last.trim_end()).into();
    kept
}

/// The key binding text for `action`, such as "Ctrl+E". Chords are joined by spaces.
fn shortcut_text(
    action: &dyn Action,
    focus: Option<&FocusHandle>,
    window: &Window,
) -> Option<SharedString> {
    let binding = match focus {
        Some(focus) => window.highest_precedence_binding_for_action_in(action, focus),
        None => window.highest_precedence_binding_for_action(action),
    }?;
    let parts: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|k| Kbd::format(k.as_keystroke()))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" ").into())
}

impl Render for TlTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let shortcut = self
            .action
            .as_ref()
            .and_then(|(action, focus)| shortcut_text(action.as_ref(), focus.as_ref(), window));

        let mut text = div().debug_selector(|| "TlTooltip-text".into());
        *text.text_style() = t_sm(&m);
        let text = text
            .text_color(theme.text_primary)
            .line_clamp(MAX_LINES)
            .text_ellipsis()
            .child(self.body_text());

        let mut panel = BaseTooltip::new("tl-tooltip")
            .flex()
            .flex_col()
            .max_w(px(self.max_width() * m.ui_scale))
            .py(m.sp[2])
            .px(m.sp[4])
            .bg(theme.menu_bg)
            .border_1()
            .border_color(theme.divider)
            .rounded(m.radius_small)
            .child(text);
        panel.style().box_shadow = Some(vec![BoxShadow {
            color: theme.derived.shadow,
            offset: point(px(0.), px(2.)),
            blur_radius: px(8.),
            spread_radius: px(0.),
            inset: false,
        }]);

        div()
            .debug_selector(|| "TlTooltip".into())
            .child(panel.when_some(shortcut, |panel, shortcut| {
                let mut line = div().debug_selector(|| "TlTooltip-shortcut".into());
                *line.text_style() = t_xs(&m);
                panel.child(
                    line.flex()
                        .justify_end()
                        .text_color(theme.text_secondary)
                        .child(shortcut),
                )
            }))
    }
}

type Builder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;

/// Keyboard-focus tooltip state for one trigger. Get it with [`FocusTooltip::state`] and wire it
/// with [`FocusTooltip::attach`] in the trigger's render.
#[derive(Default)]
pub struct FocusTooltip {
    /// The open tooltip.
    view: Option<AnyView>,
    /// The delay timer while keyboard focus rests on the trigger.
    pending: Option<Task<()>>,
    /// The delay has passed; the next render builds the view.
    due: bool,
    /// Closed by input; stays closed until focus leaves the trigger.
    dismissed: bool,
    /// The trigger's bounds from the last paint.
    trigger: Rc<Cell<Bounds<Pixels>>>,
}

impl FocusTooltip {
    /// The state kept for the trigger `id` across frames.
    pub fn state(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Entity<Self> {
        window.use_keyed_state(id, cx, |_, _| Self::default())
    }

    pub fn is_visible(&self) -> bool {
        self.view.is_some()
    }

    fn active(&self) -> bool {
        self.view.is_some() || self.pending.is_some() || self.due
    }

    fn close(&mut self, dismissed: bool) {
        self.view = None;
        self.pending = None;
        self.due = false;
        self.dismissed = dismissed;
    }

    /// Wires the keyboard tooltip into `trigger`, which must track `focus`. `build` runs once
    /// each time the tooltip opens. Call this on every render of the trigger.
    pub fn attach<E>(
        state: &Entity<Self>,
        trigger: E,
        focus: &FocusHandle,
        delay: Duration,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> E
    where
        E: InteractiveElement + ParentElement,
    {
        let focused = focus.is_focused(window);
        let keyboard = window.last_input_was_keyboard();
        let window_active = window.is_window_active();

        let build: Builder = Rc::new(build);
        let open_now = state.update(cx, |s, cx| {
            if !focused {
                s.close(false);
                return false;
            }
            if !window_active {
                s.close(true);
                return false;
            }
            if s.dismissed || s.view.is_some() {
                return false;
            }
            if s.due {
                return true;
            }
            if s.pending.is_none() && keyboard {
                s.pending = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |s, cx| {
                        s.pending = None;
                        s.due = true;
                        cx.notify();
                    });
                }));
            }
            false
        });
        if open_now {
            let view = build(window, cx);
            state.update(cx, |s, _| {
                s.due = false;
                s.view = Some(view);
            });
        }

        let (active, view, bounds) = {
            let s = state.read(cx);
            (s.active(), s.view.clone(), s.trigger.clone())
        };
        if !active {
            return trigger;
        }

        let key_state = state.clone();
        let mut trigger = trigger.capture_key_down(move |event: &KeyDownEvent, _, cx| {
            let was_visible = key_state.update(cx, |s, cx| {
                let was_visible = s.is_visible();
                s.close(true);
                cx.notify();
                was_visible
            });
            if was_visible && event.keystroke.key == "escape" {
                cx.stop_propagation();
            }
        });

        let mouse_state = state.clone();
        let bounds_writer = bounds.clone();
        trigger = trigger.child(
            canvas(
                move |b, _, _| bounds_writer.set(b),
                move |_, _, window, _| {
                    let down = mouse_state.clone();
                    window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                        if phase.bubble() {
                            down.update(cx, |s, cx| {
                                s.close(true);
                                cx.notify();
                            });
                        }
                    });
                    let scroll = mouse_state.clone();
                    window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _, cx| {
                        if phase.bubble() {
                            scroll.update(cx, |s, cx| {
                                s.close(true);
                                cx.notify();
                            });
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );

        let Some(view) = view else {
            return trigger;
        };
        let m = metrics(cx);
        trigger.child(
            deferred(
                Positioner::side(bounds.get())
                    .placement(Placement::Bottom)
                    .align(Align::Start)
                    .offset(m.sp[2])
                    .child(view),
            )
            .with_priority(PRIORITY),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        KeyBinding, Modifiers, MouseMoveEvent, StatefulInteractiveElement as _, TestAppContext,
        TestDispatcher, VisualTestContext,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;
    use crate::text::fonts::load_bundled_fonts;

    gpui_kit::actions!(tl_tooltip_test, [FocusNext, EndTask]);

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// A test context backed by the real Linux text system, so text wraps at real widths.
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

    /// A trigger with a hover tooltip, in a 1200 × 800 window.
    struct HoverHost {
        tooltip: TlTooltip,
    }

    impl Render for HoverHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size(px(1200.)).child(
                div()
                    .id("trigger")
                    .size(px(40.))
                    .tooltip(self.tooltip.clone().build())
                    .tooltip_show_delay(DELAY_CONTROL),
            )
        }
    }

    /// Hovers the trigger until the tooltip shows and returns the tooltip and text bounds.
    fn hover(cx: &mut TestAppContext, tooltip: TlTooltip) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let (_, cx) = cx.add_window_view(move |_, _| HoverHost { tooltip });
        cx.run_until_parked();
        cx.simulate_event(MouseMoveEvent {
            position: point(px(10.), px(10.)),
            pressed_button: None,
            modifiers: Modifiers::none(),
        });
        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (
            cx.debug_bounds("TlTooltip").expect("tooltip shown"),
            cx.debug_bounds("TlTooltip-text")
                .expect("tooltip text drawn"),
        )
    }

    fn line_height(cx: &mut TestAppContext) -> f32 {
        cx.update(|cx| {
            let m = metrics(cx);
            f32::from(m.text.sm) * m.line_height
        })
    }

    #[test]
    fn text_wraps_at_360() {
        let mut cx = cosmic_cx();
        let sentence = "Ends the process immediately with SIGKILL. ".repeat(14);
        let sentence = &sentence[..600];
        let (tip, text) = hover(&mut cx, TlTooltip::text(sentence.to_owned()));
        assert!(tip.size.width <= px(360.), "width {:?}", tip.size.width);
        assert!(
            text.size.height > px(line_height(&mut cx) * 2.),
            "the sentence wraps: {:?}",
            text.size
        );
    }

    #[test]
    fn lines_wrap_at_560() {
        let mut cx = cosmic_cx();
        let path: String = (0..40).map(|i| format!("/segment{i:02} ")).collect();
        let path = &path[..400];
        let (tip, text) = hover(
            &mut cx,
            TlTooltip::lines(vec!["firefox (PID 4242)".into(), path.to_owned().into()]),
        );
        assert!(tip.size.width <= px(560.), "width {:?}", tip.size.width);
        assert!(tip.size.width > px(360.), "lines use the wider limit");
        assert!(
            text.size.height > px(line_height(&mut cx) * 2.5),
            "the path line wraps: {:?}",
            text.size
        );
    }

    #[test]
    fn twelve_line_cut() {
        let lines: Vec<SharedString> = (1..=15).map(|i| format!("Line {i}").into()).collect();
        let cut = cut_lines(&lines);
        assert_eq!(cut.len(), 12);
        assert_eq!(cut[11].as_ref(), "Line 12…");
        assert_eq!(cut[10].as_ref(), "Line 11");
        assert_eq!(
            cut_lines(&lines[..12]),
            lines[..12].to_vec(),
            "12 lines stay"
        );

        let mut cx = cosmic_cx();
        let (_, text) = hover(&mut cx, TlTooltip::lines(lines));
        let lh = line_height(&mut cx);
        // GPUI snaps the line height to device pixels, so count rows with a rounding margin.
        let rows = (f32::from(text.size.height) / lh).round();
        assert_eq!(rows, 12., "12 rows show: {:?}", text.size);
    }

    #[gpui_kit::test]
    fn action_line_shows_binding(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            cx.bind_keys([KeyBinding::new("delete", EndTask, None)]);
        });
        let cx = cx.add_empty_window();
        let (bound, unbound) = cx.update(|window, _| {
            (
                shortcut_text(&EndTask, None, window),
                shortcut_text(&FocusNext, None, window),
            )
        });
        let expected = Kbd::format(&gpui_kit::Keystroke::parse("delete").unwrap());
        assert_eq!(bound.as_deref(), Some(expected.as_str()));
        assert_eq!(unbound, None);
    }

    #[test]
    fn kind_prefix() {
        assert_eq!(
            with_kind_prefix("Error", "Could not end task").as_ref(),
            "Error: Could not end task"
        );
    }

    /// A focusable button with a keyboard tooltip.
    struct FocusHost {
        root: FocusHandle,
        button: FocusHandle,
        state: Option<Entity<FocusTooltip>>,
        builds: Rc<Cell<usize>>,
    }

    impl Render for FocusHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let state = FocusTooltip::state("button-tip", window, cx);
            self.state = Some(state.clone());
            let builds = self.builds.clone();
            let button = div()
                .id("button")
                .track_focus(&self.button)
                .size(px(60.))
                .tooltip(TlTooltip::text("End task").build());
            let button = FocusTooltip::attach(
                &state,
                button,
                &self.button,
                DELAY_CONTROL,
                move |_, cx| {
                    builds.set(builds.get() + 1);
                    TlTooltip::text("End task").view(cx)
                },
                window,
                cx,
            );
            div()
                .track_focus(&self.root)
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .size(px(400.))
                .child(button)
        }
    }

    fn visible(cx: &mut VisualTestContext, view: &Entity<FocusHost>) -> bool {
        cx.update(|_, cx| {
            let state = view.read(cx).state.clone().expect("rendered");
            state.read(cx).is_visible()
        })
    }

    #[gpui_kit::test]
    fn escape_hides_keeps_focus(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            cx.bind_keys([KeyBinding::new("tab", FocusNext, None)]);
        });
        let builds = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let builds = builds.clone();
            move |_, cx| FocusHost {
                root: cx.focus_handle(),
                button: cx.focus_handle().tab_stop(true),
                state: None,
                builds,
            }
        });
        cx.update(|window, cx| {
            window.activate_window();
            view.read(cx).root.clone().focus(window, cx);
        });
        cx.run_until_parked();

        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(
                view.read(cx).button.is_focused(window),
                "Tab focuses the button"
            );
        });
        assert!(!visible(cx, &view), "no tooltip before the delay");

        cx.executor().advance_clock(DELAY_CONTROL);
        cx.run_until_parked();
        assert!(visible(cx, &view), "the tooltip shows after the delay");
        assert_eq!(builds.get(), 1, "the text is built once, when it opens");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let tip = cx.debug_bounds("TlTooltip").expect("tooltip drawn");
        assert_eq!(tip.top(), px(64.), "below the 60 px button, sp2 away");
        assert_eq!(
            tip.left(),
            px(4.),
            "left-aligned, kept 4 px inside the window"
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!visible(cx, &view), "Esc hides the tooltip");
        cx.update(|window, cx| {
            assert!(view.read(cx).button.is_focused(window), "focus stays");
        });

        cx.executor().advance_clock(DELAY_CONTROL * 2);
        cx.run_until_parked();
        assert!(!visible(cx, &view), "it stays hidden while focus rests");
    }
}
