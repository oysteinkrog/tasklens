//! Edit Layout mode of the Overview page (UI-PRF-018 "Edit layout mode", UI-PRF-056, D-023,
//! D-030).
//!
//! The page menu item "Edit Layout" turns it on. A 40 px toolbar shows at the top of the page
//! (Add Box, Reset Layout, Done), and each box gets a grip before its title and a Hide box
//! button at the right end of its title line. Both sit in the title line's side margins, drawn
//! by the page over the cached box views, so the boxes stay live and never re-render for edit
//! mode. "Add Box" puts the kind at its kind minimum in the first free place on the grid. Every
//! edit action saves the layout.
//!
//! The functions on [`OverviewLayout`] here are pure, so the rules are tested without a window.
//! [`EditState`] and the `OverviewPage` methods hold the view side.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, SharedString, Styled as _, Window,
    canvas, div, px,
};
use smallvec::SmallVec;

use super::OverviewPage;
use super::layout::{BoxKind, GridBox, GridMetrics, OverviewLayout, PlacedBox};
use super::parts::EditChrome;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::focus::TlFocusRing;
use crate::components::icons::TlIcon;
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{FIXED_KEY_BASE, ToastKey, ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::i18n::tr;
use crate::pages::performance::actions::{
    PerfOverviewAddBox, PerfOverviewEditDone, PerfOverviewEditLayout, PerfOverviewHideBox,
    PerfOverviewResetLayout,
};
use crate::pages::performance::page::PAGE_PAD;
use crate::theme::{current, metrics};

/// The toolbar height, logical px.
pub const TOOLBAR_H: f32 = 40.;

/// The box title line under the 1 px top border, logical px (the frame's `TITLE_ROW`).
const TITLE_H: f32 = 24.;

/// The grip and the Hide box button, logical px.
const HANDLE: f32 = 16.;

/// The "At least one box must stay on the Overview page" toast.
pub const LAST_BOX_TOAST: ToastKey = ToastKey(FIXED_KEY_BASE | 0x0403);

/// Debug selectors and element ids.
pub const TOOLBAR_ID: &str = "perf-overview-edit-toolbar";
pub const ADD_BUTTON_ID: &str = "perf-overview-add-box";
pub const RESET_BUTTON_ID: &str = "perf-overview-reset-layout";
pub const DONE_BUTTON_ID: &str = "perf-overview-edit-done";

/// The debug selector of `kind`'s title line handle in edit mode.
pub fn title_handle_id(kind: BoxKind) -> String {
    format!("perf-overview-edit-title-{}", kind.key())
}

/// The debug selector of `kind`'s Hide box button.
pub fn hide_button_id(kind: BoxKind) -> String {
    format!("perf-overview-hide-{}", kind.key())
}

/// The debug selector of `kind`'s grip in Edit Layout mode.
pub fn grip_id(kind: BoxKind) -> String {
    format!("perf-overview-grip-{}", kind.key())
}

/// Removes `kind` from the grid; its cells become free.
pub fn hide(layout: &mut OverviewLayout, kind: BoxKind) -> bool {
    let before = layout.boxes.len();
    layout.boxes.retain(|b| b.kind != kind);
    layout.boxes.len() != before
}

/// The kinds the layout does not place, in [`BoxKind::ALL`] order: the "Add Box" menu.
pub fn hidden_kinds(layout: &OverviewLayout) -> SmallVec<[BoxKind; 6]> {
    BoxKind::ALL
        .into_iter()
        .filter(|k| !layout.has(*k))
        .collect()
}

/// Puts `kind` at its kind minimum on `grid` in the first free place, scanning rows top to
/// bottom and columns left to right (UI-PRF-056).
pub fn add(layout: &mut OverviewLayout, kind: BoxKind, grid: &GridMetrics) -> bool {
    if layout.has(kind) {
        return false;
    }
    let (cols, rows) = grid.min_span(kind);
    let (col, row) = layout.first_free(cols, rows);
    layout.boxes.push(GridBox {
        kind,
        col,
        row,
        cols,
        rows,
    });
    true
}

/// Edit Layout mode state, kept by [`OverviewPage`] while the mode is on.
pub struct EditState {
    /// One focus handle per kind in [`BoxKind::ALL`] order; Tab moves between the placed ones.
    focus: [FocusHandle; 6],
    /// The Add Box button's bounds from the last paint, to open its menu below it.
    add_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl EditState {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            add_bounds: Rc::default(),
        }
    }

    /// The focus handle of `kind`'s box.
    pub fn focus_of(&self, kind: BoxKind) -> &FocusHandle {
        let i = BoxKind::ALL
            .iter()
            .position(|k| *k == kind)
            .expect("every kind is in ALL");
        &self.focus[i]
    }

    /// The kind whose box has focus.
    fn focused(&self, window: &Window) -> Option<BoxKind> {
        BoxKind::ALL
            .into_iter()
            .zip(&self.focus)
            .find(|(_, f)| f.is_focused(window))
            .map(|(k, _)| k)
    }
}

/// The name of `kind` in the Add Box menu.
fn kind_name(kind: BoxKind, cx: &App) -> SharedString {
    tr(
        cx,
        match kind {
            BoxKind::Cpu => "perf-card-heading-cpu",
            BoxKind::Memory => "perf-card-heading-memory",
            BoxKind::Gpu => "perf-overview-box-gpu",
            BoxKind::Disk => "perf-overview-box-disks",
            BoxKind::Net => "perf-overview-box-network",
            BoxKind::TopProcs => "perf-overview-box-processes",
        },
    )
}

impl OverviewPage {
    /// Whether Edit Layout mode is on.
    pub fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// `PerfOverviewEditLayout`: turns the mode on and takes focus, so Esc reaches the page.
    pub(super) fn on_edit_layout(
        &mut self,
        _: &PerfOverviewEditLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit.is_none() {
            self.edit = Some(EditState::new(cx));
            self.set_edit_chrome(true, cx);
            self.notify(cx);
        }
        window.focus(&self.focus, cx);
    }

    /// `PerfOverviewEditDone` (Done, Esc): turns the mode off. Focus on a box goes to the page.
    pub(super) fn on_edit_done(
        &mut self,
        _: &PerfOverviewEditDone,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        if edit.focused(window).is_some() {
            window.focus(&self.focus, cx);
        }
        self.set_edit_chrome(false, cx);
        self.notify(cx);
    }

    /// Tells the box frames whether the edit controls take the right end of the title line, and
    /// redraws every box, which only re-renders for its own changes otherwise.
    fn set_edit_chrome(&self, on: bool, cx: &mut Context<Self>) {
        cx.set_global(EditChrome(on));
        self.cpu.update(cx, |_, cx| cx.notify());
        self.mem.update(cx, |_, cx| cx.notify());
        self.gpu.update(cx, |_, cx| cx.notify());
        self.disk.update(cx, |_, cx| cx.notify());
        self.net.update(cx, |_, cx| cx.notify());
        if let Some(procs) = &self.procs {
            procs.update(cx, |_, cx| cx.notify());
        }
    }

    /// `PerfOverviewResetLayout`: the default layout, saved; the mode stays on.
    pub(super) fn on_reset_layout(
        &mut self,
        _: &PerfOverviewResetLayout,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edit.is_some() {
            self.commit(OverviewLayout::grid_default(), cx);
        }
    }

    /// `PerfOverviewAddBox`: an item of the Add Box menu.
    pub(super) fn on_add_box(
        &mut self,
        a: &PerfOverviewAddBox,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = BoxKind::from_key(&a.kind).filter(|_| self.edit.is_some()) else {
            return;
        };
        let mut layout = self.layout.clone();
        if add(&mut layout, kind, &self.placement.grid) {
            self.commit(layout, cx);
        }
    }

    /// `PerfOverviewHideBox` (Delete): hides the focused box.
    pub(super) fn on_hide_box(
        &mut self,
        _: &PerfOverviewHideBox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(kind) = self.edit.as_ref().and_then(|e| e.focused(window)) {
            self.hide_box(kind, window, cx);
        }
    }

    /// Hides `kind`, or refuses with a toast when it is the only box shown.
    fn hide_box(&mut self, kind: BoxKind, window: &mut Window, cx: &mut Context<Self>) {
        if !self.placement.boxes.iter().any(|b| b.kind != kind) {
            let text = tr(cx, "perf-overview-toast-last-box");
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(ToastKind::Info, text, Some(LAST_BOX_TOAST), None, cx)
                });
            }
            return;
        }
        let mut layout = self.layout.clone();
        if !hide(&mut layout, kind) {
            return;
        }
        if self
            .edit
            .as_ref()
            .is_some_and(|e| e.focus_of(kind).is_focused(window))
        {
            window.focus(&self.focus, cx);
        }
        self.commit(layout, cx);
    }

    /// Takes an edited layout and saves it.
    fn commit(&mut self, layout: OverviewLayout, cx: &mut Context<Self>) {
        if layout == self.layout {
            return;
        }
        self.set_layout(layout, cx);
        self.save_layout(cx);
    }

    /// Button down on a title line: `kind`'s box takes focus. The press stops here, so the box
    /// below never sees it.
    fn title_down(&mut self, kind: BoxKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        cx.stop_propagation();
        window.focus(edit.focus_of(kind), cx);
        self.notify(cx);
    }

    /// The 40 px toolbar: Add Box, Reset Layout and, at the right, Done.
    pub(super) fn edit_toolbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let edit = self.edit.as_ref()?;
        let m = *metrics(cx);
        let page_focus = self.focus.clone();
        let hidden: SmallVec<[(BoxKind, SharedString); 6]> = hidden_kinds(&self.layout)
            .into_iter()
            .map(|k| (k, kind_name(k, cx)))
            .collect();
        let empty = hidden.is_empty();
        let bounds = Rc::clone(&edit.add_bounds);
        let menu_focus = page_focus.clone();
        // A dropdown trigger: its items dispatch `PerfOverviewAddBox` on the page.
        let add_button = TlButton::new(ADD_BUTTON_ID, TlButtonKind::Ghost)
            .size(TlButtonSize::Sm)
            .trailing_icon(TlIcon::ChevronDown)
            .icon_size(m.icon.sm)
            .label(tr(cx, "perf-overview-add-box"))
            .disabled(empty)
            .on_click(move |ev, window, cx| {
                let b = bounds.get();
                let at = if b.size.width > px(0.) {
                    b.bottom_left()
                } else {
                    ev.position()
                };
                let hidden = hidden.clone();
                let menu = TlMenu::build(window, cx, menu_focus.clone(), move |mut menu, _, _| {
                    for (kind, name) in &hidden {
                        menu = menu.item(
                            name.clone(),
                            Box::new(PerfOverviewAddBox {
                                kind: kind.key().into(),
                            }),
                        );
                    }
                    menu
                });
                open_context_menu_at(at, window, cx, menu);
            });
        let measure = {
            let bounds = Rc::clone(&edit.add_bounds);
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .inset_0()
        };
        Some(
            div()
                .debug_selector(|| TOOLBAR_ID.into())
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(TOOLBAR_H * m.ui_scale))
                .px(px(PAGE_PAD * m.ui_scale))
                // The buttons dispatch on the focused handle: keep it on the page.
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    if !page_focus.contains_focused(window, cx) {
                        window.focus(&page_focus, cx);
                    }
                })
                .child(v_flex().relative().child(add_button).child(measure))
                .child(
                    TlButton::new(RESET_BUTTON_ID, TlButtonKind::Ghost)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "perf-overview-reset-layout"))
                        .action(Box::new(PerfOverviewResetLayout)),
                )
                .child(div().flex_1())
                .child(
                    TlButton::new(DONE_BUTTON_ID, TlButtonKind::Primary)
                        .size(TlButtonSize::Sm)
                        .label(tr(cx, "perf-overview-edit-done"))
                        .action(Box::new(PerfOverviewEditDone)),
                )
                .into_any_element(),
        )
    }

    /// `el`, the placed box `p`, in edit mode: a focus stop with a keyboard focus ring.
    pub(super) fn edit_box(&self, p: &PlacedBox, el: Div, window: &Window, cx: &App) -> Div {
        let Some(edit) = &self.edit else {
            return el;
        };
        let theme = current(cx);
        let m = metrics(cx);
        let handle = edit.focus_of(p.kind).clone();
        let focus = handle.clone();
        let mut el =
            el.track_focus(&handle)
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.focus(&focus, cx);
                });
        if TlFocusRing::show(handle.is_focused(window), window) {
            el = TlFocusRing::ring(el, m.radius_card, theme);
        }
        el
    }

    /// The grip, gear and Hide box button over each box's title line, in page content px.
    pub(super) fn edit_overlays(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SmallVec<[AnyElement; 8]> {
        let mut out = SmallVec::new();
        if self.edit.is_none() {
            return out;
        }
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let tip_text = tr(cx, "perf-overview-hide-box");
        for p in &self.placement.boxes {
            let kind = p.kind;
            let b = p.bounds;
            let grip = div()
                .debug_selector(move || grip_id(kind))
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(HANDLE * s))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::GripVertical)
                        .size(px(HANDLE * s))
                        .text_color(theme.text_secondary),
                );
            let hide_id = hide_button_id(kind);
            let tip = HoverTooltip::state(SharedString::from(hide_id.clone()), window, cx);
            let hide_button = div()
                .id(SharedString::from(hide_id.clone()))
                .debug_selector(move || hide_id.clone())
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(HANDLE * s))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.hide_box(kind, window, cx);
                    }),
                )
                .child(
                    Icon::new(IconName::X)
                        .size(px(HANDLE * s))
                        .text_color(theme.text_secondary),
                );
            let hide_button = HoverTooltip::attach(
                &tip,
                hide_button,
                DELAY_CONTROL,
                TlTooltip::text(tip_text.clone()).build(),
                window,
                cx,
            );
            out.push(
                div()
                    .id(SharedString::from(title_handle_id(kind)))
                    .debug_selector(move || title_handle_id(kind))
                    .absolute()
                    .left(b.left())
                    .top(b.top())
                    .w(b.size.width)
                    .h(px(1. + TITLE_H * s))
                    // No `occlude()`: it takes the Performance tab out of the hit test, so the
                    // tab's right-press capture never opens the box menu. The left press stops
                    // at the handle (title, gear, Hide box), so the box below never sees it.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.title_down(kind, window, cx);
                        }),
                    )
                    // The handle occludes the box below, so the box menu is named here.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.title_handle_right_down(kind, cx);
                        }),
                    )
                    .child(grip)
                    .child(self.box_gear(kind, cx))
                    .child(hide_button)
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::performance::overview::layout::ROW_UNIT_DEFAULT;

    use BoxKind::{Disk, Gpu, Memory, Net, TopProcs};

    #[test]
    fn add_and_hide() {
        let grid = GridMetrics::new(1200., ROW_UNIT_DEFAULT, 1.);
        let mut layout = OverviewLayout::grid_default();
        assert_eq!(hidden_kinds(&layout).as_slice(), [TopProcs]);
        // Top processes goes under the default grid at its kind minimum, 4 × 7 at 1200 px.
        assert!(add(&mut layout, TopProcs, &grid));
        assert_eq!(
            layout.get(TopProcs),
            Some(GridBox {
                kind: TopProcs,
                col: 0,
                row: 13,
                cols: 4,
                rows: 7
            })
        );
        assert!(!add(&mut layout, TopProcs, &grid));
        // Hiding frees the cells; the boxes around keep theirs.
        assert!(hide(&mut layout, Gpu));
        assert!(!hide(&mut layout, Gpu));
        assert_eq!(layout.get(Disk).map(|b| b.col), Some(4));
        // Net comes back at 3 × 6 in the first free place: GPU's old cells.
        assert!(hide(&mut layout, Net));
        assert!(add(&mut layout, Net, &grid));
        assert_eq!(
            layout.get(Net).map(|b| (b.col, b.row, b.cols, b.rows)),
            Some((0, 7, 3, 6))
        );
        assert_eq!(layout.validate(), Ok(()));
        assert!(hide(&mut layout, Memory));
        assert_eq!(hidden_kinds(&layout).as_slice(), [Memory, Gpu]);
    }
}
