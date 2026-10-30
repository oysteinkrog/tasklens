//! The Overview box menu and the Edit Layout gear (UI-PRF-019, D-025).
//!
//! A right-click on a box's title line names the box in [`ClickedBox`]. The Performance tab
//! opens its page menu for every right-click on the page (UI-PRF-015); when a box is named, the
//! menu starts with [`box_menu`]: a "Layout" submenu with one radio item per mode the box
//! offers and, when the shown mode has one, an "Arrange" submenu (D-027), then "Reset Box", then
//! a separator. A right-click elsewhere in a box names no box and keeps the plain page menu. In
//! Edit Layout mode the title handle names the box the same way, and a gear at the right end of
//! each title line opens the same mode items.
//!
//! The items dispatch [`PerfOverviewSetBoxMode`], [`PerfOverviewSetBoxArrange`],
//! [`PerfOverviewSetBoxPowerPlacement`], [`PerfOverviewSetBoxClocks`] and
//! [`PerfOverviewResetBox`], which the page handles here: the layout takes the new mode, the
//! new arrangement or the default, the box views follow, and the layout is saved.

use gpui_kit::{
    AnyElement, App, Context, Global, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Styled as _, Window, div, px,
};

use smallvec::SmallVec;
use tl_model::settings::TimelineClick;

use super::OverviewPage;
use super::layout::{Arrange, ArrangeChoice, BoxKind, PowerPlacement};
use super::parts::{TITLE_ROW, TITLE_TOP};
use super::timeline;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::i18n::tr;
use crate::pages::performance::actions::{
    PerfOverviewResetBox, PerfOverviewSetBoxArrange, PerfOverviewSetBoxClocks,
    PerfOverviewSetBoxEngine, PerfOverviewSetBoxMode, PerfOverviewSetBoxPower,
    PerfOverviewSetBoxPowerPlacement, PerfOverviewSetTimelineClick,
};
use crate::pages::performance::cpu::PowerState;
use crate::theme::metrics;
use crate::window::setup::OpenAdditionalSetup;

/// The title line a right-click opens the box menu on: the 1 px top border, the space above the
/// title row and the title row of the box frame, logical px.
const TITLE_LINE: f32 = 1. + TITLE_TOP + TITLE_ROW;

/// The "VRAM" item of the GPU box's Engines submenu and its name in the saved `hidden` list.
pub const GPU_VRAM_ID: &str = "VRAM";

/// The gear icon, logical px.
const GEAR_ICON: f32 = 16.;

/// The gear's distance from the box's right edge: the 16 px Hide box button and a 4 px gap,
/// logical px.
const GEAR_RIGHT: f32 = 20.;

/// The debug selector of `kind`'s gear in Edit Layout mode.
pub fn gear_id(kind: BoxKind) -> String {
    format!("perf-overview-gear-{}", kind.key())
}

/// The box a right-click landed on, the mode it showed and the arrangement of that mode.
/// `power` is the CPU box's power draw strip and `power_placement` where it sits: `None` for any
/// other box. `engines` is the GPU box's "Engines" list, each name with whether it is shown;
/// empty for any other box.
#[derive(Clone, PartialEq, Debug)]
pub struct BoxMenuTarget {
    pub kind: BoxKind,
    pub mode: &'static str,
    pub arrange: Option<Arrange>,
    pub power: Option<(PowerState, bool)>,
    pub power_placement: Option<PowerPlacement>,
    /// The CPU box's Core clocks chart, on or off; `None` for any other box.
    pub clocks: Option<bool>,
    pub engines: Vec<(SharedString, bool)>,
}

/// The box whose title line took the last right-press. The Performance tab clears it when a
/// right press starts and takes it when it opens the page menu, like `ClickedEngine`.
#[derive(Default)]
pub struct ClickedBox(pub Option<BoxMenuTarget>);

impl Global for ClickedBox {}

/// One radio item per mode of the target's kind, the shown mode selected, then the "Arrange"
/// submenu when the shown mode has one.
fn mode_items(
    mut menu: TlMenu,
    target: BoxMenuTarget,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
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
    menu = power_item(menu, &target, cx);
    menu = power_placement_items(menu, &target, window, cx);
    menu = clocks_item(menu, &target, cx);
    menu = engine_items(menu, &target, window, cx);
    let Some(choice) = target.kind.arrange(target.mode) else {
        return menu;
    };
    menu.separator().submenu(
        tr(cx, "perf-overview-arrange"),
        window,
        cx,
        move |menu, _, cx| arrange_items(menu, &target, choice, cx),
    )
}

/// The GPU box's "Engines" submenu (UI-PRF-019, D-029): one check item per engine the GPU
/// reports and one for VRAM. The last checked item is disabled, so one chart always shows.
fn engine_items(menu: TlMenu, target: &BoxMenuTarget, window: &mut Window, cx: &mut App) -> TlMenu {
    if target.engines.is_empty() {
        return menu;
    }
    let kind: std::sync::Arc<str> = target.kind.key().into();
    let engines = target.engines.clone();
    menu.separator().submenu(
        tr(cx, "perf-overview-engines"),
        window,
        cx,
        move |mut menu, _, cx| {
            let shown = engines.iter().filter(|(_, on)| *on).count();
            for (name, on) in &engines {
                let label = if &**name == GPU_VRAM_ID {
                    tr(cx, "perf-overview-gpu-vram")
                } else {
                    name.clone()
                };
                menu = if *on && shown == 1 {
                    menu.item_disabled_checked(label, None)
                } else {
                    menu.check_item(
                        label,
                        *on,
                        Box::new(PerfOverviewSetBoxEngine {
                            kind: kind.clone(),
                            engine: name.to_string().into(),
                            shown: !*on,
                        }),
                    )
                };
            }
            menu
        },
    )
}

/// The CPU box's "Show power draw" check item (UI-PRF-019, D-028). Without RAPL there is no
/// item. When RAPL is unreadable the item is disabled with "Needs setup", and "Set up CPU power
/// draw…" under it opens Additional Setup, as on the CPU page.
fn power_item(menu: TlMenu, target: &BoxMenuTarget, cx: &App) -> TlMenu {
    let Some((state, on)) = target.power else {
        return menu;
    };
    let label = tr(cx, "perf-overview-show-power");
    match state {
        PowerState::NoZone => menu,
        PowerState::Readable => menu.separator().check_item(
            label,
            on,
            Box::new(PerfOverviewSetBoxPower {
                kind: target.kind.key().into(),
                on: !on,
            }),
        ),
        PowerState::NeedsSetup => {
            let tip = Some(tr(cx, "perf-tip-needs-setup"));
            let menu = menu.separator();
            let menu = if on {
                menu.item_disabled_checked(label, tip)
            } else {
                menu.item_disabled(label, tip)
            };
            menu.item(
                tr(cx, "perf-cpu-setup-power"),
                Box::new(OpenAdditionalSetup { focus_row: None }),
            )
        }
    }
}

/// The CPU box's "Show core clocks" check item (tasklens-o55.57): the Core clocks chart under
/// the CPU graph. Only in the modes with the CPU graph.
fn clocks_item(menu: TlMenu, target: &BoxMenuTarget, cx: &App) -> TlMenu {
    let Some(on) = target
        .clocks
        .filter(|_| matches!(target.mode, "chart_cores" | "chart"))
    else {
        return menu;
    };
    menu.check_item(
        tr(cx, "perf-overview-show-clocks"),
        on,
        Box::new(PerfOverviewSetBoxClocks {
            kind: target.kind.key().into(),
            on: !on,
        }),
    )
}

/// The CPU box's "Power draw position" submenu (tasklens-o55.55): across the whole box, or
/// under the CPU graph beside the core rows. Only while the strip is shown in the mode with the
/// chart and the core rows, the one mode where the two differ.
fn power_placement_items(
    menu: TlMenu,
    target: &BoxMenuTarget,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let Some(current) = target
        .power_placement
        .filter(|_| target.power == Some((PowerState::Readable, true)))
        .filter(|_| target.mode == "chart_cores")
    else {
        return menu;
    };
    let kind: std::sync::Arc<str> = target.kind.key().into();
    menu.submenu(
        tr(cx, "perf-overview-power-placement"),
        window,
        cx,
        move |mut menu, _, cx| {
            for placement in PowerPlacement::ALL {
                menu = menu.radio_item(
                    tr(cx, placement.label()),
                    placement == current,
                    Box::new(PerfOverviewSetBoxPowerPlacement {
                        kind: kind.clone(),
                        placement: placement.id().into(),
                    }),
                );
            }
            menu
        },
    )
}

/// The "Arrange" items of `choice`, the shown arrangement selected. A saved choice has
/// Automatic, Stack vertically and Side by side; a choice between two modes has the last two,
/// each switching to its mode.
fn arrange_items(
    mut menu: TlMenu,
    target: &BoxMenuTarget,
    choice: ArrangeChoice,
    cx: &App,
) -> TlMenu {
    let kind: std::sync::Arc<str> = target.kind.key().into();
    match choice {
        ArrangeChoice::Saved(_) => {
            for arrange in Arrange::ALL {
                menu = menu.radio_item(
                    tr(cx, arrange.label()),
                    target.arrange == Some(arrange),
                    Box::new(PerfOverviewSetBoxArrange {
                        kind: kind.clone(),
                        arrange: arrange.id().into(),
                    }),
                );
            }
        }
        ArrangeChoice::Modes {
            vertical,
            horizontal,
        } => {
            for (arrange, mode) in [
                (Arrange::Vertical, vertical),
                (Arrange::Horizontal, horizontal),
            ] {
                menu = menu.radio_item(
                    tr(cx, arrange.label()),
                    target.mode == mode,
                    Box::new(PerfOverviewSetBoxMode {
                        kind: kind.clone(),
                        mode: mode.into(),
                    }),
                );
            }
        }
    }
    menu
}

/// The timeline's "Click" submenu (UI-PRF-057): one radio item per click choice, `history`
/// only while recording is on.
fn click_items(menu: TlMenu, window: &mut Window, cx: &mut App) -> TlMenu {
    let current = timeline::click_mode(cx);
    let choices: SmallVec<[(TimelineClick, SharedString); 3]> = timeline::click_choices(cx)
        .into_iter()
        .map(|c| (c, tr(cx, timeline::click_label(c))))
        .collect();
    menu.submenu(
        tr(cx, "perf-overview-timeline-click"),
        window,
        cx,
        move |mut menu, _, _| {
            for (click, word) in &choices {
                menu = menu.radio_item(
                    word.clone(),
                    *click == current,
                    Box::new(PerfOverviewSetTimelineClick {
                        click: timeline::click_key(*click).into(),
                    }),
                );
            }
            menu
        },
    )
}

/// "Layout" with the mode items, "Reset Box" and a separator: the start of the page menu when
/// the right-click landed on `target`'s title line. The timeline adds its "Click" submenu.
pub fn box_menu(menu: TlMenu, target: BoxMenuTarget, window: &mut Window, cx: &mut App) -> TlMenu {
    let kind = target.kind.key().into();
    let menu = if target.kind == BoxKind::Timeline {
        click_items(menu, window, cx)
    } else {
        menu
    };
    menu.submenu(
        tr(cx, "perf-overview-box-layout"),
        window,
        cx,
        move |menu, window, cx| mode_items(menu, target.clone(), window, cx),
    )
    .item(
        tr(cx, "perf-overview-reset-box"),
        Box::new(PerfOverviewResetBox { kind }),
    )
    .separator()
}

impl OverviewPage {
    /// The box of `kind` with the mode and arrangement its layout shows.
    fn menu_target(&self, kind: BoxKind, cx: &App) -> BoxMenuTarget {
        let config = self.layout.config(kind);
        let engines = if kind == BoxKind::Gpu {
            let hidden = self.layout.hidden_gpu();
            self.gpu
                .read(cx)
                .engine_names(cx)
                .into_iter()
                .map(|name| {
                    let on = !hidden.iter().any(|h| **h == *name);
                    (name, on)
                })
                .collect()
        } else {
            Vec::new()
        };
        BoxMenuTarget {
            kind,
            mode: config.mode.id,
            arrange: config.arrange,
            power: (kind == BoxKind::Cpu).then(|| (self.cpu.read(cx).power_state(), config.power)),
            power_placement: (kind == BoxKind::Cpu).then_some(config.power_placement),
            clocks: (kind == BoxKind::Cpu).then_some(config.clocks),
            engines,
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
            cx.set_global(ClickedBox(Some(self.menu_target(kind, cx))));
        }
    }

    /// A right-press on `kind`'s title handle in Edit Layout mode. The handle covers the title
    /// line and sits above the box, so it names the box itself. The press goes on to the
    /// Performance tab, which opens the menu.
    pub(super) fn title_handle_right_down(&self, kind: BoxKind, cx: &mut App) {
        let target = self.menu_target(kind, cx);
        cx.set_global(ClickedBox(Some(target)));
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

    /// `PerfOverviewSetBoxArrange`: the box takes the arrangement and the layout is saved. An
    /// unknown value, a mode without a saved Arrange choice, or the shown arrangement does
    /// nothing.
    pub(super) fn on_set_box_arrange(
        &mut self,
        a: &PerfOverviewSetBoxArrange,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(kind), Some(arrange)) =
            (BoxKind::from_key(&a.kind), Arrange::from_id(&a.arrange))
        else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.set_arrange(kind, arrange) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewSetBoxPower`: the CPU box shows or hides its power draw strip and the layout
    /// is saved. Any other kind, or the shown state, does nothing.
    pub(super) fn on_set_box_power(
        &mut self,
        a: &PerfOverviewSetBoxPower,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.set_power(kind, a.on) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewSetBoxPowerPlacement`: the CPU box places its power draw strip and the
    /// layout is saved. Any other kind, an unknown value, or the shown placement does nothing.
    pub(super) fn on_set_box_power_placement(
        &mut self,
        a: &PerfOverviewSetBoxPowerPlacement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(kind), Some(placement)) = (
            BoxKind::from_key(&a.kind),
            PowerPlacement::from_id(&a.placement),
        ) else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.set_power_placement(kind, placement) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewSetBoxClocks`: the CPU box shows or hides its Core clocks chart and the
    /// layout is saved. Any other kind, or the shown state, does nothing.
    pub(super) fn on_set_box_clocks(
        &mut self,
        a: &PerfOverviewSetBoxClocks,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        let mut layout = self.layout.clone();
        if layout.set_clocks(kind, a.on) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewSetBoxEngine`: the GPU box shows or hides an engine chart and the layout is
    /// saved. Hiding the last shown item, another kind, or the shown state does nothing.
    pub(super) fn on_set_box_engine(
        &mut self,
        a: &PerfOverviewSetBoxEngine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        let target = self.menu_target(kind, cx);
        let mut layout = self.layout.clone();
        if !a.shown && target.engines.iter().filter(|(_, on)| *on).count() <= 1 {
            return;
        }
        if layout.set_engine_hidden(kind, &a.engine, !a.shown) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
    }

    /// `PerfOverviewResetBox`: the box's default mode, split, arrangement and no power strip, saved.
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
        let target = self.menu_target(kind, cx);
        let focus = self.focus.clone();
        let id = gear_id(kind);
        let button = TlButton::new(SharedString::from(id.clone()), TlButtonKind::Ghost)
            .size(TlButtonSize::Xs)
            .icon(TlIcon::Settings2)
            .icon_size(px(GEAR_ICON * s))
            .tooltip(tr(cx, "perf-overview-box-layout-tip"))
            .on_click(move |ev, window, cx| {
                let target = target.clone();
                let menu = TlMenu::build(window, cx, focus.clone(), move |menu, window, cx| {
                    mode_items(menu, target.clone(), window, cx)
                });
                open_context_menu_at(ev.position(), window, cx, menu);
            });
        div()
            .debug_selector(move || id.clone())
            .absolute()
            // On the title line, below the border and the space above the title.
            .top(px(1. + TITLE_TOP * s))
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
