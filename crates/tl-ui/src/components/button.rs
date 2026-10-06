//! `TlButton`: the tasklens button with seven kinds and two sizes (UI-FND-010, R-123, U-41).
//!
//! Every button carries an Action and dispatches it on the window's focused handle, so menu
//! items, keys and buttons share one command (widgets rule 10). `on_click` closures are allowed
//! only inside dialogs, for OK and Cancel. A button that opens a dialog ends its label with "…"
//! (rule 11); the owner writes that text.
//!
//! Built on the kit's unstyled base `Button`, which owns the click, Space and Enter activation,
//! focus and the button role. The styled kit `Button` is not used because it cannot meet the
//! spec:
//! - Its sizes are rem based. At rem 13 `xsmall` is 16.25 px high, `small` 19.5 px and the
//!   default 26 px, so no kit size gives 24 or 28 px (U-41, measured by
//!   `tests::heights_24_and_28_at_rem_13`). TlButton sets 24 and 28 px × ui_scale itself.
//! - Its ghost hover is `menu_hover` at 0.5, not `tab_hover`, and its outlined default button
//!   turns the border to `input` on hover.
//! - Disabled swaps colours instead of opacity 0.5, and loading fades to 0.8 and puts the
//!   spinner beside the label instead of in its place.
//!
//! Hover and pressed change at once. The focus ring shows only after keyboard input. Icon and
//! Toggle buttons must have a tooltip; its text is also their accessible name.

use std::rc::Rc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::{
    Action, App, ClickEvent, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, accesskit::Toggled, div,
    prelude::FluentBuilder as _, px, rgb,
};

use super::focus::TlFocusRing;
use super::icons::{TlIcon, icon};
use super::progress::TlSpinner;
use super::tooltip::{DELAY_CONTROL, FocusTooltip, HoverTooltip, TlTooltip};
use crate::text::style::label_style;
use crate::theme::{TlTheme, UiMetrics, current, metrics};

/// Hover lightens Primary and Danger by this much; pressed darkens by the same.
const STATE_STEP: f32 = 0.08;
/// Fill alpha of an `on` Toggle or Segment.
const ON_ALPHA: f32 = 0.25;
/// Minimum width of a button with a label, × ui_scale.
const MIN_LABEL_WIDTH: f32 = 72.;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The look of a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlButtonKind {
    /// Accent fill, white text.
    Primary,
    /// Toolbar fill, 1 px divider border.
    Secondary,
    /// Danger fill, white text.
    Danger,
    /// No fill, no border.
    Ghost,
    /// A square icon-only button. Needs a tooltip.
    Icon,
    /// A 24 px square icon toggle for toolbars. Needs a tooltip.
    Toggle { on: bool },
    /// A 24 px label toggle, such as "Overall" and "Kernel on" on the Performance page.
    Segment { on: bool },
}

impl TlButtonKind {
    fn on(self) -> bool {
        matches!(self, Self::Toggle { on: true } | Self::Segment { on: true })
    }

    /// Icon and Toggle show only an icon and need a tooltip.
    fn icon_only(self) -> bool {
        matches!(self, Self::Icon | Self::Toggle { .. })
    }
}

/// Button height. Toggle and Segment are always 24 px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlButtonSize {
    /// 24 px at 100% scale.
    Sm,
    /// 28 px at 100% scale.
    #[default]
    Md,
}

/// Colours for one state of a button.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    bg: Hsla,
    hover: Option<Hsla>,
    pressed: Hsla,
    border: Option<Hsla>,
    fg: Hsla,
}

/// `c` with lightness moved by `step`, clamped to 0..=1.
fn lightness(c: Hsla, step: f32) -> Hsla {
    Hsla {
        l: (c.l + step).clamp(0., 1.),
        ..c
    }
}

/// The colours of `kind` in `theme`.
fn look(kind: TlButtonKind, theme: &TlTheme) -> Look {
    let white: Hsla = rgb(0xFFFFFF).into();
    let clear = gpui_kit::transparent_black();
    let filled = |fill: Hsla| Look {
        bg: fill,
        hover: Some(lightness(fill, STATE_STEP)),
        pressed: lightness(fill, -STATE_STEP),
        border: None,
        fg: white,
    };
    let quiet = |bg: Hsla, border: Option<Hsla>, fg: Hsla| Look {
        bg,
        hover: Some(theme.tab_hover),
        pressed: theme.header,
        border,
        fg,
    };
    let on_fill = theme.accent.alpha(ON_ALPHA);
    match kind {
        TlButtonKind::Primary => filled(theme.accent),
        TlButtonKind::Danger => filled(theme.status.danger),
        TlButtonKind::Secondary => quiet(theme.toolbar, Some(theme.divider), theme.text_primary),
        TlButtonKind::Ghost | TlButtonKind::Icon => quiet(clear, None, theme.text_primary),
        TlButtonKind::Toggle { on: false } => quiet(clear, None, theme.text_primary),
        TlButtonKind::Toggle { on: true } => quiet(on_fill, Some(theme.accent), theme.accent),
        TlButtonKind::Segment { on: false } => {
            quiet(theme.toolbar, Some(theme.divider), theme.text_primary)
        }
        TlButtonKind::Segment { on: true } => Look {
            hover: None,
            ..quiet(on_fill, Some(theme.accent), theme.text_primary)
        },
    }
}

/// Height in px for `kind` at `size`, × ui_scale.
fn height(kind: TlButtonKind, size: TlButtonSize, m: &UiMetrics) -> f32 {
    let base = match (kind, size) {
        (TlButtonKind::Toggle { .. } | TlButtonKind::Segment { .. }, _) => 24.,
        (_, TlButtonSize::Sm) => 24.,
        (_, TlButtonSize::Md) => 28.,
    };
    base * m.ui_scale
}

/// A button. Build it with [`TlButton::new`], give it an [`Action`] with [`TlButton::action`],
/// and render it as a child.
#[derive(IntoElement)]
pub struct TlButton {
    id: ElementId,
    kind: TlButtonKind,
    size: TlButtonSize,
    label: Option<SharedString>,
    icon: Option<TlIcon>,
    icon_size: Option<Pixels>,
    icon_muted: bool,
    action: Option<Rc<dyn Action>>,
    on_click: Option<ClickHandler>,
    tooltip: Option<SharedString>,
    disabled: bool,
    loading: bool,
    pressed: bool,
    tab_stop: bool,
}

impl TlButton {
    pub fn new(id: impl Into<ElementId>, kind: TlButtonKind) -> Self {
        Self {
            id: id.into(),
            kind,
            size: TlButtonSize::Md,
            label: None,
            icon: None,
            icon_size: None,
            icon_muted: false,
            action: None,
            on_click: None,
            tooltip: None,
            disabled: false,
            loading: false,
            pressed: false,
            tab_stop: true,
        }
    }

    /// The visible text. It is also the accessible name.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An `i_md` icon, left of the label when there is one.
    pub fn icon(mut self, icon: TlIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The icon size, already scaled, such as `m.icon.sm`. Default `i_md`.
    pub fn icon_size(mut self, size: Pixels) -> Self {
        self.icon_size = Some(size);
        self
    }

    /// Draws the icon in `text_secondary`, as toolbar buttons do while their window is
    /// inactive. Default `false`.
    pub fn icon_muted(mut self, muted: bool) -> Self {
        self.icon_muted = muted;
        self
    }

    pub fn size(mut self, size: TlButtonSize) -> Self {
        self.size = size;
        self
    }

    /// The command this button runs. It is dispatched on the window's focused handle on click,
    /// Space and Enter, and its key binding shows in the tooltip.
    pub fn action(mut self, action: Box<dyn Action>) -> Self {
        self.action = Some(Rc::from(action));
        self
    }

    /// A click closure. Allowed only for OK and Cancel inside dialogs; everything else uses
    /// [`TlButton::action`]. Runs after the action is dispatched.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Tooltip text, shown on hover and after keyboard focus rests on the button. The key binding
    /// of the action shows on its own line. Required for Icon and Toggle, whose accessible name
    /// is this text.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// A disabled button is drawn at opacity 0.5, has no hover or pressed look, is not a tab
    /// stop and ignores input. Its tooltip still shows.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Replaces the label with a spinner and makes the button inert. The width stays the same.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Keeps the pressed look, for a dropdown trigger while its menu is open.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// The colour the icon is drawn with in `theme`.
    pub fn icon_color(&self, theme: &TlTheme) -> Hsla {
        if self.icon_muted {
            theme.text_secondary
        } else {
            look(self.kind, theme).fg
        }
    }

    /// Whether Tab stops on the button. Default `true`.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
}

impl RenderOnce for TlButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        debug_assert!(
            !self.kind.icon_only() || self.tooltip.is_some(),
            "TlButton {:?} needs a tooltip",
            self.kind
        );
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let look = look(self.kind, &theme);
        let h = px(height(self.kind, self.size, &m));
        let inert = self.disabled || self.loading;
        let icon_fg = self.icon_color(&theme);
        let icon_only = self.kind.icon_only() || self.label.is_none();

        let focus: FocusHandle = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let show_ring = !self.disabled && TlFocusRing::show(focus.is_focused(window), window);

        let pad = match self.kind {
            _ if icon_only => px(0.),
            TlButtonKind::Ghost => m.sp[4],
            _ => m.sp[5],
        };
        let text = match self.kind {
            TlButtonKind::Primary | TlButtonKind::Danger => {
                label_style(&m, m.text.base, FontWeight::MEDIUM)
            }
            TlButtonKind::Segment { .. } => label_style(&m, m.text.sm, FontWeight::NORMAL),
            _ => label_style(&m, m.text.base, FontWeight::NORMAL),
        };

        let mut content = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(m.sp[3])
            .when(self.loading, |c| c.invisible())
            .when_some(self.icon, |c, name| {
                let id = self.id.clone();
                c.child(
                    div()
                        .debug_selector(move || format!("TlButton-{id}-icon"))
                        .flex_none()
                        .child(icon(name, self.icon_size.unwrap_or(m.icon.md), icon_fg)),
                )
            });
        if let Some(label) = self.label.clone().filter(|_| !self.kind.icon_only()) {
            let mut text_el = div().debug_selector(|| "TlButton-label".into());
            *text_el.text_style() = text;
            content = content.child(text_el.whitespace_nowrap().child(label));
        }

        let accessible = if icon_only {
            self.tooltip.clone()
        } else {
            self.label.clone()
        };
        let action = self.action.clone();
        let on_click = self.on_click;
        let bg = if self.pressed { look.pressed } else { look.bg };

        let mut button = BaseButton::new(self.id.clone())
            .disabled(self.disabled)
            .tab_stop(self.tab_stop)
            .track_focus(&focus)
            .key_context("TlButton")
            .debug_selector({
                let id = self.id.clone();
                move || format!("TlButton-{id}")
            })
            .relative()
            .flex_none()
            .h(h)
            .when(icon_only, |b| b.w(h))
            .when(!icon_only, |b| b.min_w(px(MIN_LABEL_WIDTH * m.ui_scale)))
            .px(pad)
            .rounded(m.radius_small)
            .bg(bg)
            .text_color(look.fg)
            .when_some(look.border, |b, border| b.border_1().border_color(border))
            .when(self.disabled, |b| b.opacity(0.5))
            .when_some(accessible, |b, name| b.accessibility_label(name))
            .when(
                matches!(
                    self.kind,
                    TlButtonKind::Toggle { .. } | TlButtonKind::Segment { .. }
                ),
                |b| {
                    b.aria_toggled(if self.kind.on() {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                },
            )
            .when(!inert, |b| {
                b.when_some(look.hover.filter(|_| !self.pressed), |b, hover| {
                    b.hover(move |s| s.bg(hover))
                })
                .active(move |s| s.bg(look.pressed))
                // Clicking leaves focus where it was, so the action reaches the focused view.
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(move |event, window, cx| {
                    if let Some(action) = &action {
                        window.dispatch_action(action.boxed_clone(), cx);
                    }
                    if let Some(on_click) = &on_click {
                        on_click(event, window, cx);
                    }
                })
            })
            .child(content)
            .when(self.loading, |b| {
                b.child(
                    div()
                        .debug_selector(|| "TlButton-spinner".into())
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(TlSpinner::new(m.icon.sm).color(look.fg)),
                )
            });

        if let Some(text) = self.tooltip {
            let mut tip = TlTooltip::text(text);
            if let Some(action) = &self.action {
                tip = tip.action(action.as_ref(), None);
            }
            let hover = HoverTooltip::state((self.id.clone(), "hover-tooltip"), window, cx);
            button = HoverTooltip::attach(
                &hover,
                button,
                DELAY_CONTROL,
                tip.clone().build(),
                window,
                cx,
            );
            let state = FocusTooltip::state((self.id.clone(), "focus-tooltip"), window, cx);
            button = FocusTooltip::attach(
                &state,
                button,
                &focus,
                DELAY_CONTROL,
                tip.build(),
                window,
                cx,
            );
        }

        if show_ring {
            TlFocusRing::ring(button, m.radius_small, &theme)
        } else {
            button
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui_kit::component::Sizable as _;
    use gpui_kit::component::button::Button as KitButton;
    use gpui_kit::{
        AnyElement, Background, Context, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Pixels,
        Render, TestAppContext, VisualTestContext, point,
    };
    use tl_model::settings::{AppearanceSettings, ThemeSettings};

    use super::*;

    gpui_kit::actions!(tl_button_test, [Go]);

    type Build = Box<dyn Fn() -> AnyElement>;

    fn setup(cx: &mut App) {
        gpui_kit::init(cx);
        crate::theme::store::init(
            cx,
            &ThemeSettings::default(),
            &AppearanceSettings::default(),
            Vec::new(),
        );
    }

    /// Renders `build()` in a column and counts `Go` actions that reach the root.
    struct Host {
        build: Build,
        hits: Rc<Cell<usize>>,
        rem: Option<Pixels>,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            if let Some(rem) = self.rem {
                window.set_rem_size(rem);
            }
            let hits = self.hits.clone();
            div()
                .id("root")
                .tab_group()
                .size(px(400.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .on_action(move |_: &Go, _, _| hits.set(hits.get() + 1))
                .child((self.build)())
        }
    }

    fn host(
        cx: &mut TestAppContext,
        rem: Option<Pixels>,
        build: impl Fn() -> AnyElement + 'static,
    ) -> (
        gpui_kit::Entity<Host>,
        &mut VisualTestContext,
        Rc<Cell<usize>>,
    ) {
        cx.update(setup);
        let hits = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let hits = hits.clone();
            move |_, _| Host {
                build: Box::new(build),
                hits,
                rem,
            }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (view, cx, hits)
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
        cx.update(|window, cx| {
            assert!(window.focused(cx).is_some(), "a button takes focus");
            window.draw(cx).clear(cx);
        });
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> gpui_kit::Bounds<Pixels> {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} drawn"))
    }

    #[gpui_kit::test]
    fn dispatches_action_on_enter(cx: &mut TestAppContext) {
        let (_, cx, hits) = host(cx, None, || {
            TlButton::new("run", TlButtonKind::Primary)
                .label("Run")
                .action(Box::new(Go))
                .into_any_element()
        });
        focus_next(cx);
        press(cx, "enter");
        assert_eq!(hits.get(), 1);
    }

    #[gpui_kit::test]
    fn space_activates(cx: &mut TestAppContext) {
        let (_, cx, hits) = host(cx, None, || {
            TlButton::new("run", TlButtonKind::Secondary)
                .label("Run")
                .action(Box::new(Go))
                .into_any_element()
        });
        focus_next(cx);
        press(cx, "space");
        assert_eq!(hits.get(), 1);
    }

    fn toggle(on: bool) -> AnyElement {
        TlButton::new("pin", TlButtonKind::Toggle { on })
            .icon(TlIcon::Pin)
            .tooltip("Always on top")
            .action(Box::new(Go))
            .size(TlButtonSize::Sm)
            .into_any_element()
    }

    /// Counts painted quads: the `accent` at 0.25 fills, and the 1 px `accent` borders. GPUI
    /// paints a border as one quad per edge.
    fn accent_quads(cx: &mut VisualTestContext) -> (usize, usize) {
        cx.update(|window, cx| {
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

    #[gpui_kit::test]
    fn toggle_look(cx: &mut TestAppContext) {
        let (view, cx, _) = host(cx, None, || toggle(true));
        // Keep the pointer off the button, so it shows no hover fill.
        cx.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::none());
        let (fills, borders) = accent_quads(cx);
        assert_eq!(fills, 1, "on: accent fill at 0.25");
        assert!(borders > 0, "on: 1 px accent border");
        let b = bounds(cx, "TlButton-pin");
        assert_eq!((b.size.width, b.size.height), (px(24.), px(24.)));

        cx.update(|_, cx| {
            view.update(cx, |host, cx| {
                host.build = Box::new(|| toggle(false));
                cx.notify();
            })
        });
        assert_eq!(accent_quads(cx), (0, 0), "off: no accent fill or border");
    }

    #[gpui_kit::test]
    fn loading_keeps_width(cx: &mut TestAppContext) {
        let (view, cx, hits) = host(cx, None, || {
            TlButton::new("apply", TlButtonKind::Primary)
                .label("Apply all changes now")
                .action(Box::new(Go))
                .into_any_element()
        });
        let before = bounds(cx, "TlButton-apply");
        assert!(cx.debug_bounds("TlButton-spinner").is_none());

        cx.update(|_, cx| {
            view.update(cx, |host, cx| {
                host.build = Box::new(|| {
                    TlButton::new("apply", TlButtonKind::Primary)
                        .label("Apply all changes now")
                        .action(Box::new(Go))
                        .loading(true)
                        .into_any_element()
                });
                cx.notify();
            })
        });
        let after = bounds(cx, "TlButton-apply");
        assert_eq!(after.size, before.size, "the width is kept");
        let spinner = bounds(cx, "TlButton-spinner");
        assert_eq!(
            spinner.center(),
            after.center(),
            "the spinner takes the label's place"
        );

        cx.simulate_click(after.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(hits.get(), 0, "a loading button is inert");
    }

    #[gpui_kit::test]
    fn disabled_not_tab_stop(cx: &mut TestAppContext) {
        let clicks = Rc::new(Cell::new(0));
        let (_, cx, hits) = host(cx, None, {
            let clicks = clicks.clone();
            move || {
                let clicks = clicks.clone();
                div()
                    .child(
                        TlButton::new("off", TlButtonKind::Primary)
                            .label("Disabled")
                            .action(Box::new(Go))
                            .disabled(true),
                    )
                    .child(
                        TlButton::new("on", TlButtonKind::Secondary)
                            .label("Enabled")
                            .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                    )
                    .into_any_element()
            }
        });
        let off = bounds(cx, "TlButton-off");
        cx.simulate_click(off.center(), Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            hits.get(),
            0,
            "a click on a disabled button dispatches nothing"
        );

        focus_next(cx);
        press(cx, "enter");
        assert_eq!(clicks.get(), 1, "Tab skips the disabled button");
        assert_eq!(hits.get(), 0);
        focus_next(cx);
        press(cx, "enter");
        assert_eq!(clicks.get(), 2, "the enabled button is the only tab stop");
        assert_eq!(hits.get(), 0);
    }

    /// U-41: the kit sizes at rem 13 next to TlButton's own heights.
    #[gpui_kit::test]
    fn heights_24_and_28_at_rem_13(cx: &mut TestAppContext) {
        let (_, cx, _) = host(cx, Some(px(13.)), || {
            div()
                .flex()
                .flex_col()
                .child(
                    TlButton::new("sm", TlButtonKind::Secondary)
                        .label("Small")
                        .size(TlButtonSize::Sm),
                )
                .child(
                    TlButton::new("md", TlButtonKind::Secondary)
                        .label("Medium")
                        .size(TlButtonSize::Md),
                )
                .child(
                    KitButton::new("kit-xs")
                        .label("Kit")
                        .xsmall()
                        .debug_selector(|| "kit-xs".into()),
                )
                .child(
                    KitButton::new("kit-sm")
                        .label("Kit")
                        .small()
                        .debug_selector(|| "kit-sm".into()),
                )
                .child(
                    KitButton::new("kit-md")
                        .label("Kit")
                        .debug_selector(|| "kit-md".into()),
                )
                .into_any_element()
        });
        assert_eq!(bounds(cx, "TlButton-sm").size.height, px(24.));
        assert_eq!(bounds(cx, "TlButton-md").size.height, px(28.));
        // No kit size reaches 24 or 28 px at rem 13 (16.25, 19.5 and 26 before pixel snapping),
        // so TlButton sets its own heights.
        for (selector, rem_height) in [("kit-xs", 16.25), ("kit-sm", 19.5), ("kit-md", 26.)] {
            let h = f32::from(bounds(cx, selector).size.height);
            assert!((h - rem_height).abs() <= 0.5, "{selector}: {h}");
        }
    }

    #[gpui_kit::test]
    fn icon_muted_uses_text_secondary(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            let t = current(cx).clone();
            let on = TlButton::new("pin", TlButtonKind::Toggle { on: true }).icon(TlIcon::Pin);
            assert_eq!(on.icon_color(&t), t.accent, "default: the kind's colour");
            assert_eq!(on.icon_muted(true).icon_color(&t), t.text_secondary);
        });
    }

    #[gpui_kit::test]
    fn looks_follow_the_table(cx: &mut TestAppContext) {
        cx.update(|cx| {
            setup(cx);
            let t = current(cx).clone();
            let primary = look(TlButtonKind::Primary, &t);
            assert_eq!(primary.bg, t.accent);
            assert_eq!(primary.hover, Some(lightness(t.accent, 0.08)));
            assert_eq!(primary.pressed, lightness(t.accent, -0.08));
            assert_eq!(look(TlButtonKind::Danger, &t).bg, t.status.danger);
            let secondary = look(TlButtonKind::Secondary, &t);
            assert_eq!(secondary.border, Some(t.divider));
            assert_eq!(secondary.hover, Some(t.tab_hover));
            assert_eq!(secondary.pressed, t.header);
            let seg_on = look(TlButtonKind::Segment { on: true }, &t);
            assert_eq!(seg_on.bg, t.accent.alpha(0.25));
            assert_eq!(seg_on.border, Some(t.accent));
            assert_eq!(seg_on.hover, None, "Segment hovers only when off");
        });
    }
}
