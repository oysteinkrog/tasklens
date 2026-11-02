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
//!
//! When a box hosts a widget editor (UI-PRF-061: the GPU box, and from phase 2 the CPU, Memory,
//! Disk, Network, Top processes and Timeline boxes),
//! its menu has "Edit Widgets…" above "Reset Box" ([`PerfWidgetEditBox`]; the GPU box also keeps
//! [`PerfWidgetEditStart`]), which ends Edit Layout and starts Edit Widgets. While the box shows
//! the user's own widgets (a grid layout), "Arrange", "Engines", "Show power draw", "Power draw
//! position" and "Show core clocks" are hidden, a "Layout" item asks before it replaces the
//! widgets, and "Reset Box" asks before it saves.

use gpui_kit::{
    AnyElement, App, Context, Entity, Global, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, SharedString, Styled as _, Window, div, px,
};

use smallvec::SmallVec;
use tl_ui_settings::layout::{
    CPU_BOX_CONTAINER, DISK_BOX_CONTAINER, GPU_BOX_CONTAINER, MEMORY_BOX_CONTAINER,
    NET_BOX_CONTAINER, PROCESSES_BOX_CONTAINER, TIMELINE_BOX_CONTAINER, reset_to_default,
};
use tl_ui_settings::layout_boxes::{convert_box, fixed_box_default};
use tl_ui_settings::settings::TimelineClick;

use super::OverviewPage;
use super::layout::{Arrange, ArrangeChoice, BoxKind, PowerPlacement};
use super::parts::{TITLE_ROW, TITLE_TOP};
use super::timeline;
use crate::actions::OpenAdditionalSetup;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::pages::performance::core::actions::{
    PerfOverviewEditDone, PerfOverviewResetBox, PerfOverviewSetBoxArrange,
    PerfOverviewSetBoxClocks, PerfOverviewSetBoxEngine, PerfOverviewSetBoxMode,
    PerfOverviewSetBoxPower, PerfOverviewSetBoxPowerPlacement, PerfOverviewSetTimelineClick,
    PerfWidgetEditBox, PerfWidgetEditStart,
};
use crate::pages::performance::cpu::PowerState;
use crate::pages::performance::widget::edit::{WidgetEditEvent, WidgetEditor, editor_of};
use crate::pages::performance::widget::menu::{confirm_preset, confirm_reset_now};
use crate::selector::TlSelector;
use crate::settings_global::TlSettings;
use crate::theme::metrics;

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

/// The debug selector of the cover over `kind` while a box is in Edit Widgets: the whole box,
/// or the title line of the box being edited.
pub fn widget_edit_cover_id(kind: BoxKind) -> String {
    format!("perf-overview-widget-edit-cover-{}", kind.key())
}

/// The debug selector of the cover over the range header while a box is in Edit Widgets.
pub const WIDGET_EDIT_HEADER_COVER_ID: &str = "perf-overview-widget-edit-cover-header";

/// A cover over part of the page while a box is in Edit Widgets (UI-PRF-061). The part under
/// it stays live and drawn, but takes no pointer input except the wheel, so the page still
/// scrolls. It also keeps the Performance tab's right-press from opening a menu.
pub(super) fn widget_edit_cover(id: String) -> gpui_kit::Div {
    div()
        .tl_selector(move || id)
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .block_mouse_except_scroll()
}

/// The cover over `kind`'s title line, when `kind` is the box in Edit Widgets. Its body holds
/// the editor and stays open to the pointer.
pub(super) fn widget_edit_title_cover(kind: BoxKind, cx: &App) -> gpui_kit::Div {
    widget_edit_cover(widget_edit_cover_id(kind)).h(px(TITLE_LINE * metrics(cx).ui_scale))
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
    /// The box hosts a widget editor: the menu offers "Edit Widgets…" (UI-PRF-061).
    pub edit_widgets: bool,
    /// The box shows the user's own widgets (a grid layout): "Arrange", "Engines" and the CPU
    /// box's switches are hidden and the "Layout" items are plain items that ask first.
    pub widget_grid: bool,
}

/// The widget container of the box of `kind`, when the box is on widgets: the GPU box
/// (phase 1), the CPU box (tasklens-6swdr.4), the Memory box (tasklens-6swdr.5), the Disk box
/// (tasklens-6swdr.6), the Network box (tasklens-6swdr.7), and the Top processes and Timeline
/// boxes (tasklens-6swdr.8).
pub fn widget_container(kind: BoxKind) -> Option<&'static str> {
    match kind {
        BoxKind::Gpu => Some(GPU_BOX_CONTAINER),
        BoxKind::Cpu => Some(CPU_BOX_CONTAINER),
        BoxKind::Memory => Some(MEMORY_BOX_CONTAINER),
        BoxKind::Disk => Some(DISK_BOX_CONTAINER),
        BoxKind::Net => Some(NET_BOX_CONTAINER),
        BoxKind::TopProcs => Some(PROCESSES_BOX_CONTAINER),
        BoxKind::Timeline => Some(TIMELINE_BOX_CONTAINER),
    }
}

/// The kinds of the boxes on widgets, in page order.
const WIDGET_BOXES: [BoxKind; 7] = [
    BoxKind::Cpu,
    BoxKind::Memory,
    BoxKind::Gpu,
    BoxKind::Disk,
    BoxKind::Net,
    BoxKind::TopProcs,
    BoxKind::Timeline,
];

/// Whether `container`'s saved layout is a grid layout.
fn grid_layout(container: &str, cx: &App) -> bool {
    cx.has_global::<TlSettings>()
        && TlSettings::settings(cx)
            .performance
            .layouts
            .known(container)
            .is_some_and(|l| l.preset.is_none())
}

/// The "Edit Widgets…" item's action for the box of `kind`.
fn edit_action(kind: BoxKind) -> Box<dyn gpui_kit::Action> {
    Box::new(PerfWidgetEditBox {
        kind: kind.key().into(),
    })
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
        let action = Box::new(PerfOverviewSetBoxMode {
            kind: kind.clone(),
            mode: mode.id.into(),
        });
        // Over the user's own widgets no preset is selected; choosing one asks first.
        menu = if target.widget_grid {
            menu.item(tr(cx, mode.label), action)
        } else {
            menu.radio_item(tr(cx, mode.label), mode.id == target.mode, action)
        };
    }
    menu = power_item(menu, &target, cx);
    menu = power_placement_items(menu, &target, window, cx);
    menu = clocks_item(menu, &target, cx);
    menu = engine_items(menu, &target, window, cx);
    let Some(choice) = target
        .kind
        .arrange(target.mode)
        .filter(|_| !target.widget_grid)
    else {
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
fn click_items(
    menu: TlMenu,
    hub: &Entity<SnapshotHub>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let current = timeline::click_mode(Some(hub), cx);
    let choices: SmallVec<[(TimelineClick, SharedString); 3]> =
        timeline::click_choices(Some(hub), cx)
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
/// the right-click landed on `target`'s title line. The timeline adds its "Click" submenu, from
/// the settings of `hub`'s host.
pub fn box_menu(
    menu: TlMenu,
    target: BoxMenuTarget,
    hub: &Entity<SnapshotHub>,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let kind = target.kind.key().into();
    let edit_widgets = target.edit_widgets;
    let box_kind = target.kind;
    let menu = if target.kind == BoxKind::Timeline {
        click_items(menu, hub, window, cx)
    } else {
        menu
    };
    let menu = menu.submenu(
        tr(cx, "perf-overview-box-layout"),
        window,
        cx,
        move |menu, window, cx| mode_items(menu, target.clone(), window, cx),
    );
    let menu = if edit_widgets {
        menu.item(tr(cx, "perf-widget-edit"), edit_action(box_kind))
    } else {
        menu
    };
    menu.item(
        tr(cx, "perf-overview-reset-box"),
        Box::new(PerfOverviewResetBox { kind }),
    )
    .separator()
}

impl OverviewPage {
    /// The box of `kind` with the mode and arrangement its layout shows.
    fn menu_target(&self, kind: BoxKind, cx: &App) -> BoxMenuTarget {
        let config = self.layout.config(kind);
        let container = widget_container(kind);
        let edit_widgets = container.is_some_and(|c| editor_of(&self.hub, c, cx).is_some());
        let widget_grid = edit_widgets && container.is_some_and(|c| grid_layout(c, cx));
        // The CPU box's switches edit its flow layout; over a grid layout they are hidden.
        let cpu = kind == BoxKind::Cpu && !widget_grid;
        let engines = if kind == BoxKind::Gpu && !widget_grid {
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
            power: cpu.then(|| (self.cpu.read(cx).power_state(), config.power)),
            power_placement: cpu.then_some(config.power_placement),
            clocks: cpu.then_some(config.clocks),
            engines,
            edit_widgets,
            widget_grid,
        }
    }

    /// The box in Edit Widgets, if any. The page renders again when an editor starts or ends
    /// Edit Widgets, so the covers follow.
    pub(super) fn widget_edit_box(&mut self, cx: &mut Context<Self>) -> Option<BoxKind> {
        let mut editing = None;
        for kind in WIDGET_BOXES {
            let Some(editor) = widget_container(kind).and_then(|c| editor_of(&self.hub, c, cx))
            else {
                continue;
            };
            self.watch_widget_editor(&editor, cx);
            if editing.is_none() && editor.read(cx).is_editing() {
                editing = Some(kind);
            }
        }
        editing
    }

    /// Listens to `editor`'s start and end, once per editor.
    fn watch_widget_editor(&mut self, editor: &Entity<WidgetEditor>, cx: &mut Context<Self>) {
        let id = editor.entity_id();
        if self
            .widget_edit_subs
            .iter()
            .any(|(watched, _)| *watched == id)
        {
            return;
        }
        let sub = cx.subscribe(editor, |this, _, _: &WidgetEditEvent, cx| this.notify(cx));
        self.widget_edit_subs.push((id, sub));
    }

    /// `PerfWidgetEditStart`: the GPU box starts Edit Widgets, as [`Self::start_widget_edit`].
    pub(super) fn on_widget_edit_start(
        &mut self,
        _: &PerfWidgetEditStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_widget_edit(BoxKind::Gpu, window, cx);
    }

    /// `PerfWidgetEditBox`: the box of `kind` starts Edit Widgets, as
    /// [`Self::start_widget_edit`].
    pub(super) fn on_widget_edit_box(
        &mut self,
        a: &PerfWidgetEditBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(kind) = BoxKind::from_key(&a.kind) {
            self.start_widget_edit(kind, window, cx);
        }
    }

    /// Edit Layout ends as "Done" ends it, then the box of `kind` starts Edit Widgets. Without a
    /// widget editor on that box, or while another box is in Edit Widgets, it does nothing.
    fn start_widget_edit(&mut self, kind: BoxKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = widget_container(kind).and_then(|c| editor_of(&self.hub, c, cx)) else {
            return;
        };
        if self.widget_edit_box(cx).is_some_and(|k| k != kind) {
            return;
        }
        self.watch_widget_editor(&editor, cx);
        if self.is_editing() {
            self.on_edit_done(&PerfOverviewEditDone, window, cx);
        }
        match kind {
            BoxKind::Gpu => self.gpu.update(cx, |g, cx| g.prepare_edit(cx)),
            BoxKind::Cpu => self.cpu.update(cx, |b, cx| b.prepare_edit(cx)),
            BoxKind::Memory => self.mem.update(cx, |b, cx| b.prepare_edit(cx)),
            BoxKind::Disk => self.disk.update(cx, |b, cx| b.prepare_edit(cx)),
            BoxKind::Net => self.net.update(cx, |b, cx| b.prepare_edit(cx)),
            BoxKind::TopProcs => {
                if let Some(p) = &self.procs {
                    p.frame.update(cx, |b, cx| b.prepare_edit(cx));
                }
            }
            BoxKind::Timeline => self.timeline.frame.update(cx, |b, cx| b.prepare_edit(cx)),
        }
        editor.update(cx, |e, cx| e.start(window, cx));
        self.notify(cx);
    }

    /// "Replace your widgets?" confirmed: the box of `kind` takes preset `mode`, saved as its
    /// flow layout and as the old `mode` key.
    fn replace_widgets(&mut self, kind: BoxKind, mode: &str, cx: &mut Context<Self>) {
        let mut layout = self.layout.clone();
        if layout.set_mode(kind, mode) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
        if !cx.has_global::<TlSettings>() {
            return;
        }
        TlSettings::update(cx, |d| {
            let perf = &mut d.settings.performance;
            let mut entry = perf
                .overview
                .as_ref()
                .and_then(|o| o.grid.iter().find(|e| e.kind == kind.key()))
                .cloned()
                .unwrap_or_default();
            entry.mode = Some(mode.to_owned());
            if let Some(flow) =
                convert_box(kind.key(), &entry).or_else(|| fixed_box_default(kind.key()))
            {
                perf.layouts.set(flow);
            }
        });
    }

    /// Reset Box: the box's default mode, split, arrangement and no power strip, saved. A box on
    /// widgets also saves its default widget layout (ADR-019 "Saving the default"), so its old
    /// keys do not convert again at the next start.
    fn reset_box_now(&mut self, kind: BoxKind, cx: &mut Context<Self>) {
        let mut layout = self.layout.clone();
        if layout.reset_box(kind) {
            self.set_layout(layout, cx);
            self.save_layout(cx);
        }
        let Some(container) = widget_container(kind) else {
            return;
        };
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let mut perf = TlSettings::settings(cx).performance.clone();
        reset_to_default(&mut perf, container);
        if perf != TlSettings::settings(cx).performance {
            TlSettings::update(cx, |d| {
                reset_to_default(&mut d.settings.performance, container);
            });
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        if let Some(container) =
            widget_container(kind).filter(|_| self.menu_target(kind, cx).widget_grid)
        {
            let Some(mode) = kind.modes().iter().find(|m| *m.id == *a.mode) else {
                return;
            };
            let (id, label) = (mode.id, tr(cx, mode.label));
            let page = cx.entity().downgrade();
            confirm_preset(container, label, window, cx, move |_, cx| {
                let _ = page.update(cx, |p, cx| p.replace_widgets(kind, id, cx));
            });
            return;
        }
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

    /// `PerfOverviewResetBox`: the box's default mode, split, arrangement and no power strip,
    /// saved. Over the user's own widgets in a box it asks first, since they cannot be
    /// restored.
    pub(super) fn on_reset_box(
        &mut self,
        a: &PerfOverviewResetBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind) else {
            return;
        };
        if let Some(container) =
            widget_container(kind).filter(|_| self.menu_target(kind, cx).widget_grid)
        {
            let page = cx.entity().downgrade();
            confirm_reset_now(container, window, cx, move |_, cx| {
                let _ = page.update(cx, |p, cx| p.reset_box_now(kind, cx));
            });
            return;
        }
        self.reset_box_now(kind, cx);
    }

    /// The gear at the right end of `kind`'s title line in Edit Layout mode, left of Hide box.
    /// It opens the box's mode items below the pointer.
    pub(super) fn box_gear(&self, kind: BoxKind, cx: &App) -> AnyElement {
        let s = metrics(cx).ui_scale;
        let target = self.menu_target(kind, cx);
        let edit_widgets = target.edit_widgets;
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
                    let menu = mode_items(menu, target.clone(), window, cx);
                    if edit_widgets {
                        menu.separator()
                            .item(tr(cx, "perf-widget-edit"), edit_action(kind))
                    } else {
                        menu
                    }
                });
                open_context_menu_at(ev.position(), window, cx, menu);
            });
        div()
            .tl_selector(move || id.clone())
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
