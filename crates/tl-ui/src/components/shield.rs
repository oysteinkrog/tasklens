//! `TlShield`: the toolbar button that shows and toggles the privileged-helper state
//! (UI-FND-028, APP-003, ADR-006).
//!
//! The owner reads `snapshot.health.helper` and re-renders on the `HEALTH` bit. The shield only
//! dispatches `StartHelperSession` and `EndHelperSession`; the engine runs pkexec (ADR-006) and
//! the shell pushes the failure toasts (E29).
//!
//! It is drawn and behaves as a 24 × 24 `TlButton::Toggle`, but is built on the kit's unstyled
//! base `Button`, because `TlButton` takes only a Lucide icon tinted with the button text colour.
//! The shield needs the app SVG tinted `status.success` when unlocked and a `TlSpinner` `i_md` in
//! place of the icon while starting. The colours below are the Toggle colours of `TlButton`.

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::Icon;
use gpui_kit::{
    Action, App, ElementId, Empty, FocusHandle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window,
    accesskit::Toggled, div, gpui, prelude::FluentBuilder as _, px,
};
use tl_model::health::HelperState;

use super::focus::TlFocusRing;
use super::icons::{AppIcon, TlIcon, icon};
use super::progress::TlSpinner;
use super::tooltip::{DELAY_CONTROL, FocusTooltip, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::theme::{TlTheme, current, metrics};

/// Starts the privileged helper session: the engine runs pkexec, with a 120 s polkit deadline
/// (ADR-006). Also dispatched by the "Retry as administrator" toast button (PROC-058). E20's
/// `actions.rs` re-exports it instead of declaring it again (R-128).
#[derive(Clone, PartialEq, Default, gpui::Action)]
#[action(namespace = tasklens)]
pub struct StartHelperSession;

/// Ends the privileged helper session. Re-exported by E20's `actions.rs` (R-128).
#[derive(Clone, PartialEq, Default, gpui::Action)]
#[action(namespace = tasklens)]
pub struct EndHelperSession;

/// Fill alpha of the `on` Toggle look.
const ON_ALPHA: f32 = 0.25;
/// Toggle size at 100% scale.
const SIZE: f32 = 24.;
const ID: &str = "tl-shield";

/// What sits in the button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    /// Lucide `shield` in `text_primary`.
    Shield,
    /// `TlSpinner` in place of the icon.
    Spinner,
    /// App SVG `shield-unlocked.svg` in `status.success`.
    Unlocked,
}

/// The rendered form of one helper state.
#[derive(Clone, Copy, Debug)]
struct Face {
    glyph: Glyph,
    /// The Toggle `on` look.
    on: bool,
    /// Fluent key of the tooltip, which is also the accessible name.
    tooltip: &'static str,
    /// Dispatched on click, Space and Enter. `None` makes the button inert.
    action: Option<fn() -> Box<dyn Action>>,
}

/// The face for `state`. `None` for `Missing`: the shield is not drawn.
fn face(state: HelperState) -> Option<Face> {
    match state {
        HelperState::Missing => None,
        HelperState::Locked => Some(Face {
            glyph: Glyph::Shield,
            on: false,
            tooltip: "fnd-shield-unlock",
            action: Some(|| Box::new(StartHelperSession)),
        }),
        HelperState::Starting => Some(Face {
            glyph: Glyph::Spinner,
            on: false,
            tooltip: "fnd-shield-waiting",
            action: None,
        }),
        HelperState::Unlocked => Some(Face {
            glyph: Glyph::Unlocked,
            on: true,
            tooltip: "fnd-shield-unlocked",
            action: Some(|| Box::new(EndHelperSession)),
        }),
    }
}

/// Glyph colour: `status.success` for the unlocked SVG, the Toggle text colour otherwise.
fn glyph_color(glyph: Glyph, theme: &TlTheme) -> Hsla {
    match glyph {
        Glyph::Unlocked => theme.status.success,
        Glyph::Shield | Glyph::Spinner => theme.text_primary,
    }
}

/// The privileged-helper button. Render it in the toolbar slot for every state; it draws
/// nothing for `Missing`.
#[derive(IntoElement)]
pub struct TlShield {
    state: HelperState,
    focus: Option<FocusHandle>,
}

impl TlShield {
    pub fn new(state: HelperState) -> Self {
        Self { state, focus: None }
    }

    /// Uses the owner's focus handle, for a toolbar that moves keyboard focus onto its buttons.
    /// Without it the shield keeps its own handle. It is never a tab stop (UI-FND-010).
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

impl RenderOnce for TlShield {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(face) = face(self.state) else {
            return Empty.into_any_element();
        };
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let size = px(SIZE * m.ui_scale);
        let id = ElementId::from(ID);
        let tooltip: SharedString = tr(cx, face.tooltip);
        let action: Option<Box<dyn Action>> = face.action.map(|make| make());

        let focus = self.focus.unwrap_or_else(|| {
            window
                .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone()
        });
        let show_ring = TlFocusRing::show(focus.is_focused(window), window);

        let color = glyph_color(face.glyph, &theme);
        let glyph = match face.glyph {
            Glyph::Shield => div()
                .debug_selector(|| "TlShield-icon-shield".into())
                .child(icon(TlIcon::Shield, m.icon.md, color)),
            Glyph::Spinner => div()
                .debug_selector(|| "TlShield-spinner".into())
                .child(TlSpinner::new(m.icon.md).color(color)),
            Glyph::Unlocked => div()
                .debug_selector(|| "TlShield-icon-unlocked".into())
                .child(
                    Icon::new(AppIcon::ShieldUnlocked)
                        .size(m.icon.md)
                        .text_color(color),
                ),
        };

        let (bg, border) = if face.on {
            (theme.accent.alpha(ON_ALPHA), Some(theme.accent))
        } else {
            (gpui_kit::transparent_black(), None)
        };
        let hover = theme.tab_hover;
        let pressed = theme.header;
        let dispatch = action.as_ref().map(|a| a.boxed_clone());

        let mut button = BaseButton::new(id.clone())
            .tab_stop(false)
            .track_focus(&focus)
            .key_context("TlShield")
            .debug_selector(|| "TlShield".into())
            .relative()
            .flex_none()
            .size(size)
            .rounded(m.radius_small)
            .bg(bg)
            .when_some(border, |b, border| b.border_1().border_color(border))
            .accessibility_label(tooltip.clone())
            .aria_toggled(if face.on {
                Toggled::True
            } else {
                Toggled::False
            })
            .when_some(dispatch, |b, action| {
                b.hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    // Clicking leaves focus where it was, so the action reaches the focused view.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx);
                    })
            })
            .child(glyph);

        let mut tip = TlTooltip::text(tooltip);
        if let Some(action) = &action {
            tip = tip.action(action.as_ref(), None);
        }
        let hover_state = HoverTooltip::state((id.clone(), "hover-tooltip"), window, cx);
        button = HoverTooltip::attach(
            &hover_state,
            button,
            DELAY_CONTROL,
            tip.clone().build(),
            window,
            cx,
        );
        let focus_state = FocusTooltip::state((id, "focus-tooltip"), window, cx);
        button = FocusTooltip::attach(
            &focus_state,
            button,
            &focus,
            DELAY_CONTROL,
            tip.build(),
            window,
            cx,
        );

        if show_ring {
            TlFocusRing::ring(button, m.radius_small, &theme).into_any_element()
        } else {
            button.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::{
        Background, Context, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render,
        TestAppContext, VisualTestContext, point,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    const STATES: [HelperState; 4] = [
        HelperState::Missing,
        HelperState::Locked,
        HelperState::Starting,
        HelperState::Unlocked,
    ];

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
        crate::i18n::init_strings(cx, "en-US");
    }

    /// Renders the shield for `state` and counts the helper actions that reach the root.
    struct Host {
        state: HelperState,
        /// The focused view, as in the app: a click dispatches on it.
        root: FocusHandle,
        /// The shield's handle, for keyboard focus.
        focus: FocusHandle,
        starts: Rc<Cell<usize>>,
        ends: Rc<Cell<usize>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let starts = self.starts.clone();
            let ends = self.ends.clone();
            div()
                .id("root")
                .track_focus(&self.root)
                .size(px(200.))
                .flex()
                .items_start()
                .on_action(move |_: &StartHelperSession, _, _| starts.set(starts.get() + 1))
                .on_action(move |_: &EndHelperSession, _, _| ends.set(ends.get() + 1))
                .child(TlShield::new(self.state).track_focus(&self.focus))
        }
    }

    struct Rig<'a> {
        view: Entity<Host>,
        cx: &'a mut VisualTestContext,
        starts: Rc<Cell<usize>>,
        ends: Rc<Cell<usize>>,
    }

    impl Rig<'_> {
        fn draw(&mut self) {
            self.cx.update(|window, cx| window.draw(cx).clear(cx));
        }

        fn set(&mut self, state: HelperState) {
            self.cx.update(|_, cx| {
                self.view.update(cx, |host, cx| {
                    host.state = state;
                    cx.notify();
                })
            });
            self.draw();
        }

        fn has(&mut self, selector: &'static str) -> bool {
            self.draw();
            self.cx.debug_bounds(selector).is_some()
        }

        fn click(&mut self) {
            let b = self.cx.debug_bounds("TlShield").expect("shield drawn");
            self.cx.simulate_click(b.center(), Modifiers::none());
            self.cx.run_until_parked();
        }

        /// A click where the shield would be.
        fn click_at_origin(&mut self) {
            self.cx
                .simulate_click(point(px(12.), px(12.)), Modifiers::none());
            self.cx.run_until_parked();
        }

        fn press(&mut self, key: &str) {
            let keystroke = Keystroke::parse(key).unwrap();
            self.cx.simulate_event(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            self.cx.simulate_event(KeyUpEvent { keystroke });
            self.cx.run_until_parked();
        }

        /// The `accent` at 0.25 fills and the 1 px `accent` border edges painted.
        fn accent_quads(&mut self) -> (usize, usize) {
            self.cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                let theme = current(cx).clone();
                let one = window.scale_factor();
                let quads = window.painted_quads();
                let fills = quads
                    .iter()
                    .filter(|q| q.background == Background::from(theme.accent.alpha(ON_ALPHA)))
                    .count();
                let borders = quads
                    .iter()
                    .filter(|q| q.border_color == theme.accent && q.border_widths.top.0 == one)
                    .count();
                (fills, borders)
            })
        }
    }

    fn rig(cx: &mut TestAppContext, state: HelperState) -> Rig<'_> {
        cx.update(setup);
        let starts = Rc::new(Cell::new(0));
        let ends = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let starts = starts.clone();
            let ends = ends.clone();
            move |_, cx| Host {
                state,
                root: cx.focus_handle(),
                focus: cx.focus_handle(),
                starts,
                ends,
            }
        });
        let mut rig = Rig {
            view,
            cx,
            starts,
            ends,
        };
        // Keep the pointer off the button, so it shows no hover fill.
        rig.cx
            .simulate_mouse_move(point(px(150.), px(150.)), None, Modifiers::none());
        let root = rig.cx.update(|_, cx| rig.view.read(cx).root.clone());
        rig.cx.update(|window, cx| window.focus(&root, cx));
        rig.draw();
        rig
    }

    #[gpui_kit::test]
    fn hidden_without_helper(cx: &mut TestAppContext) {
        assert!(face(HelperState::Missing).is_none());
        let mut rig = rig(cx, HelperState::Missing);
        assert!(!rig.has("TlShield"), "no element for Missing");
        rig.click_at_origin();
        assert_eq!((rig.starts.get(), rig.ends.get()), (0, 0));
    }

    #[gpui_kit::test]
    fn states_and_tooltips(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            let theme = current(cx).clone();
            let expect = [
                (
                    HelperState::Locked,
                    Glyph::Shield,
                    false,
                    "Unlock privileged actions",
                    theme.text_primary,
                ),
                (
                    HelperState::Starting,
                    Glyph::Spinner,
                    false,
                    "Waiting for authentication…",
                    theme.text_primary,
                ),
                (
                    HelperState::Unlocked,
                    Glyph::Unlocked,
                    true,
                    "Privileged actions unlocked (click to lock)",
                    theme.status.success,
                ),
            ];
            for (state, glyph, on, text, color) in expect {
                let f = face(state).expect("drawn");
                assert_eq!((f.glyph, f.on), (glyph, on), "{state:?}");
                assert_eq!(tr(cx, f.tooltip).as_ref(), text, "{state:?}");
                assert_eq!(glyph_color(f.glyph, &theme), color, "{state:?}");
            }
        });

        let selectors = [
            "TlShield-icon-shield",
            "TlShield-spinner",
            "TlShield-icon-unlocked",
        ];
        let mut rig = rig(cx, HelperState::Locked);
        for (state, shown, on) in [
            (HelperState::Locked, "TlShield-icon-shield", false),
            (HelperState::Starting, "TlShield-spinner", false),
            (HelperState::Unlocked, "TlShield-icon-unlocked", true),
        ] {
            rig.set(state);
            for selector in selectors {
                assert_eq!(
                    rig.has(selector),
                    selector == shown,
                    "{state:?}: {selector}"
                );
            }
            let b = rig.cx.debug_bounds("TlShield").expect("drawn");
            assert_eq!((b.size.width, b.size.height), (px(24.), px(24.)));
            let (fills, borders) = rig.accent_quads();
            if on {
                assert_eq!(fills, 1, "{state:?}: accent fill at 0.25");
                assert!(borders > 0, "{state:?}: 1 px accent border");
            } else {
                assert_eq!((fills, borders), (0, 0), "{state:?}: off look");
            }
        }
    }

    #[gpui_kit::test]
    fn click_dispatches_per_state(cx: &mut TestAppContext) {
        let mut rig = rig(cx, HelperState::Locked);
        rig.click();
        assert_eq!((rig.starts.get(), rig.ends.get()), (1, 0), "Locked starts");

        rig.set(HelperState::Starting);
        rig.click();
        assert_eq!(
            (rig.starts.get(), rig.ends.get()),
            (1, 0),
            "Starting is inert"
        );
        assert!(rig.has("TlShield-spinner"));

        rig.set(HelperState::Unlocked);
        rig.click();
        assert_eq!((rig.starts.get(), rig.ends.get()), (1, 1), "Unlocked ends");

        // Space and Enter with keyboard focus on the shield.
        let focus = rig.cx.update(|_, cx| rig.view.read(cx).focus.clone());
        rig.cx.update(|window, cx| window.focus(&focus, cx));
        rig.draw();
        rig.press("space");
        assert_eq!((rig.starts.get(), rig.ends.get()), (1, 2), "Space ends");

        rig.set(HelperState::Locked);
        rig.press("enter");
        assert_eq!((rig.starts.get(), rig.ends.get()), (2, 2), "Enter starts");

        rig.set(HelperState::Starting);
        rig.press("space");
        assert_eq!(
            (rig.starts.get(), rig.ends.get()),
            (2, 2),
            "Starting ignores keys"
        );
    }

    #[test]
    fn every_state_has_a_face_but_missing() {
        for state in STATES {
            assert_eq!(face(state).is_some(), state != HelperState::Missing);
        }
        let actions: Vec<_> = STATES
            .into_iter()
            .filter_map(face)
            .map(|f| f.action.map(|make| make().name()))
            .collect();
        assert_eq!(
            actions,
            [
                Some("tasklens::StartHelperSession"),
                None,
                Some("tasklens::EndHelperSession")
            ]
        );
    }
}
