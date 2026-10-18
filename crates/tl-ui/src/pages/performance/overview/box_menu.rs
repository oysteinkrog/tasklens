//! The Overview box menu and the Edit Layout gear (UI-PRF-019, D-025).
//!
//! A right-click on a box's title line names the box in [`ClickedBox`]. The Performance tab
//! opens its page menu for every right-click on the page (UI-PRF-015); when a box is named, the
//! menu starts with [`box_menu`]: a "Layout" submenu with one radio item per mode the box
//! offers, then "Reset Box", then a separator. A right-click elsewhere in a box names no box and
//! keeps the plain page menu. In Edit Layout mode a gear at the right end of each title line
//! opens the same mode items.
//!
//! The items dispatch [`PerfOverviewSetBoxMode`] and [`PerfOverviewResetBox`], which the page
//! handles here: the layout takes the new mode or the default, the box views follow, and the
//! layout is saved.

use gpui_kit::{
    AnyElement, App, Context, Global, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Styled as _, Window, div, px,
};

use super::OverviewPage;
use super::layout::BoxKind;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::i18n::tr;
use crate::pages::performance::actions::{PerfOverviewResetBox, PerfOverviewSetBoxMode};
use crate::theme::metrics;

/// The title line a right-click opens the box menu on: the 1 px top border and the 24 px title
/// row of the box frame, logical px.
const TITLE_LINE: f32 = 25.;

/// The gear icon, logical px.
const GEAR_ICON: f32 = 16.;

/// The gear's distance from the box's right edge: the 16 px Hide box button and a 4 px gap,
/// logical px.
const GEAR_RIGHT: f32 = 20.;

/// The debug selector of `kind`'s gear in Edit Layout mode.
pub fn gear_id(kind: BoxKind) -> String {
    format!("perf-overview-gear-{}", kind.key())
}

/// The box a right-click landed on, and the mode it showed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxMenuTarget {
    pub kind: BoxKind,
    pub mode: &'static str,
}

/// The box whose title line took the last right-press. The Performance tab clears it when a
/// right press starts and takes it when it opens the page menu, like `ClickedEngine`.
#[derive(Default)]
pub struct ClickedBox(pub Option<BoxMenuTarget>);

impl Global for ClickedBox {}

/// One radio item per mode of the target's kind, the shown mode selected.
fn mode_items(mut menu: TlMenu, target: BoxMenuTarget, cx: &App) -> TlMenu {
    let kind: std::sync::Arc<str> = target.kind.key().into();
    for mode in target.kind.modes() {
        menu = menu.radio_item(
            tr(cx, mode.label),
            mode.id == target.mode,
            Box::new(PerfOverviewSetBoxMode {
                kind: kind.clone(),
                mode: mode.id.into(),
            }),
        );
    }
    menu
}

/// "Layout" with the mode items, "Reset Box" and a separator: the start of the page menu when
/// the right-click landed on `target`'s title line.
pub fn box_menu(menu: TlMenu, target: BoxMenuTarget, window: &mut Window, cx: &mut App) -> TlMenu {
    menu.submenu(
        tr(cx, "perf-overview-box-layout"),
        window,
        cx,
        move |menu, _, cx| mode_items(menu, target, cx),
    )
    .item(
        tr(cx, "perf-overview-reset-box"),
        Box::new(PerfOverviewResetBox {
            kind: target.kind.key().into(),
        }),
    )
    .separator()
}

impl OverviewPage {
    /// The box of `kind` with the mode its layout shows.
    fn menu_target(&self, kind: BoxKind) -> BoxMenuTarget {
        BoxMenuTarget {
            kind,
            mode: self.layout.config(kind).mode.id,
        }
    }

    /// A right-press on the box of `kind`: on its title line it names the box for the page
    /// menu. The press goes on to the Performance tab, which opens the menu.
    pub(super) fn box_right_down(&mut self, kind: BoxKind, ev: &MouseDownEvent, cx: &mut App) {
        let Some(placed) = self.placement.boxes.iter().find(|b| b.kind == kind) else {
            return;
        };
        let y = ev.position.y - self.content_origin.get().y - placed.bounds.top();
        if f32::from(y) <= TITLE_LINE * metrics(cx).ui_scale {
            cx.set_global(ClickedBox(Some(self.menu_target(kind))));
        }
    }

    /// `PerfOverviewSetBoxMode`: the box takes the mode and the layout is saved. A mode the box
    /// does not offer, or the one it shows, does nothing.
    pub(super) fn on_set_box_mode(
        &mut self,
        a: &PerfOverviewSetBoxMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.set_mode(kind, &a.mode) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewResetBox`: the box's default mode and split, saved.
    pub(super) fn on_reset_box(
        &mut self,
        a: &PerfOverviewResetBox,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.reset_box(kind) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// The gear at the right end of `kind`'s title line in Edit Layout mode, left of Hide box.
    /// It opens the box's mode items below the pointer.
    pub(super) fn box_gear(&self, kind: BoxKind, cx: &App) -> AnyElement {
        let s = metrics(cx).ui_scale;
        let target = self.menu_target(kind);
        let focus = self.focus.clone();
        let id = gear_id(kind);
        let button = TlButton::new(SharedString::from(id.clone()), TlButtonKind::Ghost)
            .size(TlButtonSize::Xs)
            .icon(TlIcon::Settings2)
            .icon_size(px(GEAR_ICON * s))
            .tooltip(tr(cx, "perf-overview-box-layout-tip"))
            .on_click(move |ev, window, cx| {
                let menu = TlMenu::build(window, cx, focus.clone(), move |menu, _, cx| {
                    mode_items(menu, target, cx)
                });
                open_context_menu_at(ev.position(), window, cx, menu);
            });
        div()
            .debug_selector(move || id.clone())
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(GEAR_RIGHT * s))
            .flex()
            .items_center()
            // A press on the gear is not the start of a box drag.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button)
            .into_any_element()
    }
}
