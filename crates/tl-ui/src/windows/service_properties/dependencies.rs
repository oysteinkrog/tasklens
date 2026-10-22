//! Service Properties Dependencies tab (UI-SYS-035, SYS-014).
//!
//! What the unit depends on, what depends on it, and (collapsed) the ordering relations: one
//! row per (unit, relation) with a status dot and a relation tag. The window pushes each new
//! details Arc in through `set_details`; the tab rebuilds its rows then and notifies itself,
//! because the window draws it through `.cached()`. The dots follow the services list, which the
//! tab reads from the hub, so they change without new details. The tab is never dirty.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, EntityId, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px, uniform_list,
};
use smallvec::SmallVec;
use tl_model::actions::ActionId;
use tl_model::snapshot::{ComponentMask, Relation, UnitDetails, UnitRecord};
use tl_model::table::UnitRef;
use tracing::warn;

use super::{ServicePropertiesWindow, ServicePropsTab, ServicePropsTabView};
use crate::components::badge::{TlBadge, TlBadgeKind};
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::{TlMenu, open_context_menu_at};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::{tr, tr_args};
use crate::pages::services::actions::ServicePropsOk;
use crate::pages::services::delegate::service_sys_status;
use crate::pages::sys_status::{
    DotStyle, LabelTone, StatusLook, cell_colors, status_label, status_look,
};
use crate::text::style::{label_style, t_base, t_xs};
use crate::theme::{current, metrics};
use fluent_bundle::FluentValue;

/// A block with more rows than this is drawn through `uniform_list`.
const LIST_ROWS: usize = 200;

gpui_kit::actions!(tasklens, [ServiceDepsCopyValue]);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Block {
    DependsOn,
    Dependents,
    Ordering,
}

fn block(r: Relation) -> Block {
    match r {
        Relation::Requires
        | Relation::Requisite
        | Relation::Wants
        | Relation::BindsTo
        | Relation::PartOf
        | Relation::Upholds => Block::DependsOn,
        Relation::RequiredBy
        | Relation::RequisiteOf
        | Relation::WantedBy
        | Relation::BoundBy
        | Relation::ConsistsOf
        | Relation::UpheldBy => Block::Dependents,
        Relation::After | Relation::Before => Block::Ordering,
    }
}

/// The systemd name of a relation. These are property names, not translated.
fn relation_word(r: Relation) -> &'static str {
    match r {
        Relation::Requires => "Requires",
        Relation::Requisite => "Requisite",
        Relation::Wants => "Wants",
        Relation::BindsTo => "BindsTo",
        Relation::PartOf => "PartOf",
        Relation::Upholds => "Upholds",
        Relation::RequiredBy => "RequiredBy",
        Relation::RequisiteOf => "RequisiteOf",
        Relation::WantedBy => "WantedBy",
        Relation::BoundBy => "BoundBy",
        Relation::ConsistsOf => "ConsistsOf",
        Relation::UpheldBy => "UpheldBy",
        Relation::After => "After",
        Relation::Before => "Before",
    }
}

/// One (unit, relation) row.
struct DepRow {
    unit: UnitRef,
    relation: Relation,
    id: SharedString,
    look: StatusLook,
    /// The status word, or "Not loaded".
    status: SharedString,
}

/// One line of the tab, every line `row_height` high.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    Heading(Block),
    Empty,
    /// An index into `rows`.
    Row(usize),
    OrderingHeader,
}

/// What is selected: a row by (unit, relation), so it survives a rebuild, or the header.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Sel {
    Row(Arc<str>, Relation),
    Header,
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the service Dependencies tab"),
    }
}

pub struct DependenciesTab {
    entity_id: EntityId,
    details: Option<Arc<UnitDetails>>,
    units: Arc<[UnitRecord]>,
    /// Depends on rows, then Dependents rows, then Ordering rows, each sorted by unit id.
    rows: Vec<DepRow>,
    /// Row counts of the three blocks.
    counts: [usize; 3],
    items: Vec<Item>,
    expanded: bool,
    selected: Option<Sel>,
    /// The unit id "Copy value" copies.
    menu_value: Option<SharedString>,
    heading_depends: SharedString,
    heading_dependents: SharedString,
    none: SharedString,
    collecting: SharedString,
    not_loaded: SharedString,
    ordering: SharedString,
    focus: FocusHandle,
    scroll: gpui_kit::UniformListScrollHandle,
    _sub: Option<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
}

impl DependenciesTab {
    pub(super) fn new(cx: &mut Context<Self>) -> Self {
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        let units = hub
            .as_ref()
            .map(|hub| hub.read(cx).current().services.units.clone())
            .unwrap_or_else(|| Arc::from([]));
        let sub = hub.as_ref().map(|hub| {
            cx.subscribe(hub, |this, hub, event: &SnapshotChanged, cx| {
                if event.mask.intersects(ComponentMask::SERVICES) {
                    let units = hub.read(cx).current().services.units.clone();
                    this.take_units(units, cx);
                }
            })
        });
        let mut this = Self {
            entity_id: cx.entity_id(),
            details: None,
            units,
            rows: Vec::new(),
            counts: [0; 3],
            items: Vec::new(),
            expanded: false,
            selected: None,
            menu_value: None,
            heading_depends: tr(cx, "services-sys035-depends-on"),
            heading_dependents: tr(cx, "services-sys035-dependents"),
            none: tr(cx, "services-sys035-none"),
            collecting: tr(cx, "fnd-kv-collecting"),
            not_loaded: tr(cx, "services-sys035-not-loaded"),
            ordering: SharedString::default(),
            focus: cx.focus_handle(),
            scroll: gpui_kit::UniformListScrollHandle::new(),
            _sub: sub,
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
        };
        this.rebuild(cx);
        this
    }

    /// The dot look and status word of `unit` in the current services list.
    fn status_of(&self, unit: &UnitRef, cx: &mut App) -> (StatusLook, SharedString) {
        match self
            .units
            .iter()
            .find(|r| r.unit == *unit && &*r.load_state == "loaded")
        {
            Some(record) => {
                let status = service_sys_status(record.status);
                (status_look(status), status_label(cx, status))
            }
            None => (
                StatusLook {
                    dot: DotStyle::Ring(LabelTone::Secondary),
                    label: LabelTone::Secondary,
                },
                self.not_loaded.clone(),
            ),
        }
    }

    fn take_units(&mut self, units: Arc<[UnitRecord]>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&self.units, &units) {
            return;
        }
        self.units = units;
        let mut changed = false;
        for i in 0..self.rows.len() {
            let (look, status) = self.status_of(&self.rows[i].unit, cx);
            let row = &mut self.rows[i];
            if row.look != look || row.status != status {
                row.look = look;
                row.status = status;
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    /// Splits the details into the three sorted blocks and lays out the items.
    fn rebuild(&mut self, cx: &mut App) {
        let mut rows = Vec::new();
        let mut counts = [0; 3];
        if let Some(details) = self.details.clone() {
            for (slot, which) in [Block::DependsOn, Block::Dependents, Block::Ordering]
                .into_iter()
                .enumerate()
            {
                let mut entries: Vec<_> = details
                    .deps
                    .iter()
                    .filter(|e| block(e.relation) == which)
                    .collect();
                entries.sort_by(|a, b| a.unit.name.cmp(&b.unit.name));
                // The same unit under the same relation is one row; two relations stay two
                // rows, each with its own tag.
                entries.dedup_by(|a, b| a.unit == b.unit && a.relation == b.relation);
                counts[slot] = entries.len();
                for e in entries {
                    let (look, status) = self.status_of(&e.unit, cx);
                    rows.push(DepRow {
                        id: SharedString::from(e.unit.name.to_string()),
                        unit: e.unit.clone(),
                        relation: e.relation,
                        look,
                        status,
                    });
                }
            }
        }
        self.rows = rows;
        self.counts = counts;
        self.ordering = tr_args(
            cx,
            "services-sys035-ordering",
            &[("n", FluentValue::from(counts[2] as i64))],
        );
        self.layout();
        let keep = match &self.selected {
            Some(Sel::Row(name, relation)) => self
                .rows
                .iter()
                .any(|r| r.unit.name == *name && r.relation == *relation),
            Some(Sel::Header) => true,
            None => true,
        };
        if !keep || self.details.is_none() {
            self.selected = None;
        }
    }

    /// The items from `rows`, `counts` and `expanded`.
    fn layout(&mut self) {
        self.items.clear();
        if self.details.is_none() {
            return;
        }
        let mut next = 0;
        for (slot, which) in [Block::DependsOn, Block::Dependents]
            .into_iter()
            .enumerate()
        {
            self.items.push(Item::Heading(which));
            if self.counts[slot] == 0 {
                self.items.push(Item::Empty);
            }
            for _ in 0..self.counts[slot] {
                self.items.push(Item::Row(next));
                next += 1;
            }
        }
        self.items.push(Item::OrderingHeader);
        if self.expanded {
            for _ in 0..self.counts[2] {
                self.items.push(Item::Row(next));
                next += 1;
            }
        }
    }

    fn sel_of(&self, item: Item) -> Option<Sel> {
        match item {
            Item::Row(i) => {
                let r = &self.rows[i];
                Some(Sel::Row(r.unit.name.clone(), r.relation))
            }
            Item::OrderingHeader => Some(Sel::Header),
            Item::Heading(_) | Item::Empty => None,
        }
    }

    fn selected_item(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.items
            .iter()
            .position(|&item| self.sel_of(item).as_ref() == Some(sel))
    }

    fn select_item(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&item) = self.items.get(ix) else {
            return;
        };
        self.selected = self.sel_of(item);
        self.scroll
            .scroll_to_item(ix, gpui_kit::ScrollStrategy::Nearest);
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let selectable: Vec<usize> = (0..self.items.len())
            .filter(|&ix| self.sel_of(self.items[ix]).is_some())
            .collect();
        if selectable.is_empty() {
            return;
        }
        let target = match self.selected_item() {
            None => selectable[0],
            Some(cur) => {
                let pos = selectable.iter().position(|&ix| ix == cur).unwrap_or(0) as isize;
                selectable[(pos + delta).clamp(0, selectable.len() as isize - 1) as usize]
            }
        };
        self.select_item(target, cx);
    }

    fn toggle_ordering(&mut self, cx: &mut Context<Self>) {
        self.expanded = !self.expanded;
        self.layout();
        cx.notify();
    }

    /// Enter or double-click on `item`: open a row's unit, toggle the header.
    fn activate(&mut self, item: Item, cx: &mut Context<Self>) {
        match item {
            Item::Row(i) => {
                let unit = self.rows[i].unit.clone();
                // Defer: opening or focusing a window must not run inside this window's update.
                cx.defer(move |cx| {
                    ServicePropertiesWindow::open(unit, ServicePropsTab::General, cx)
                });
            }
            Item::OrderingHeader => self.toggle_ordering(cx),
            Item::Heading(_) | Item::Empty => {}
        }
    }

    fn activate_selected(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selected_item().map(|ix| self.items[ix]) {
            Some(item) => {
                self.activate(item, cx);
                true
            }
            None => false,
        }
    }

    fn on_click(
        &mut self,
        ix: usize,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.select_item(ix, cx);
        let item = self.items[ix];
        match item {
            Item::OrderingHeader => {
                if ev.click_count() == 1 {
                    self.toggle_ordering(cx);
                }
            }
            Item::Row(_) if ev.click_count() >= 2 => self.activate(item, cx),
            _ => {}
        }
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let mods = ks.modifiers;
        if mods.control || mods.alt || mods.platform || mods.function || mods.shift {
            return;
        }
        match ks.key.as_str() {
            "up" | "down" => {
                self.move_selection(if ks.key == "up" { -1 } else { 1 }, cx);
                cx.stop_propagation();
            }
            "enter" => {
                if self.activate_selected(cx) {
                    cx.stop_propagation();
                }
            }
            "space" if self.selected == Some(Sel::Header) => {
                self.toggle_ordering(cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    /// Enter is bound to the window's OK. On a selected line it acts on the line instead.
    fn on_enter(&mut self, _: &ServicePropsOk, _: &mut Window, cx: &mut Context<Self>) {
        if !self.activate_selected(cx) {
            cx.propagate();
        }
    }

    fn open_value_menu(
        &mut self,
        ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Item::Row(i) = self.items[ix] else {
            return;
        };
        window.focus(&self.focus, cx);
        self.select_item(ix, cx);
        self.menu_value = Some(self.rows[i].id.clone());
        let label = tr(cx, "services-sys035-copy-value");
        let menu = TlMenu::build(window, cx, self.focus.clone(), |menu, _, _| {
            menu.item(label, Box::new(ServiceDepsCopyValue))
        });
        open_context_menu_at(position, window, cx, menu);
    }

    fn on_copy_value(
        &mut self,
        _: &ServiceDepsCopyValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.menu_value.take() else {
            return;
        };
        if crate::util::clipboard_write(cx, text.to_string()).is_ok() {
            let toast = tr(cx, "sys-sys006-copied-to-clipboard");
            window_toast(window, cx, ToastKind::Success, toast);
        }
    }

    /// `margins` is false in a `uniform_list`, where every item must be one height.
    fn item_element(&mut self, ix: usize, margins: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let selected = self.selected_item() == Some(ix);
        let item = self.items[ix];
        let first = ix == 0 || !margins;
        match item {
            Item::Heading(which) => {
                let text = match which {
                    Block::DependsOn => self.heading_depends.clone(),
                    _ => self.heading_dependents.clone(),
                };
                let mut d = div()
                    .flex_none()
                    .h(m.row_height)
                    .flex()
                    .items_end()
                    .pb(m.sp[3])
                    .px(m.sp[4])
                    .text_color(theme.text_primary)
                    .child(text);
                *d.text_style() = label_style(&m, m.text.base, FontWeight::SEMIBOLD);
                if first {
                    d.into_any_element()
                } else {
                    // The top margin is part of the item so every item stays one height.
                    d.mt(m.sp[6]).into_any_element()
                }
            }
            Item::Empty => {
                let mut d = div()
                    .flex_none()
                    .h(m.row_height)
                    .flex()
                    .items_center()
                    .px(m.sp[4])
                    .text_color(theme.text_secondary)
                    .child(self.none.clone());
                *d.text_style() = t_base(&m);
                d.into_any_element()
            }
            Item::OrderingHeader => {
                let chevron = if self.expanded {
                    TlIcon::ChevronDown
                } else {
                    TlIcon::ChevronRight
                };
                let mut d = h_flex()
                    .id("ServiceDeps-ordering")
                    .debug_selector(|| "ServiceDeps-ordering".into())
                    .flex_none()
                    .h(m.row_height)
                    .px(m.sp[4])
                    .gap(m.sp[3])
                    .items_center()
                    .when(margins, |d| d.mt(m.sp[6]))
                    .text_color(theme.text_primary)
                    .hover(|s| s.bg(theme.row_hover))
                    .role(Role::Button)
                    .aria_label(self.ordering.clone())
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        this.on_click(ix, ev, window, cx)
                    }))
                    .child(icon(chevron, m.icon.md, theme.text_secondary))
                    .child(self.ordering.clone());
                if selected {
                    d = d.bg(theme.row_selected);
                }
                *d.text_style() = t_base(&m);
                d.into_any_element()
            }
            Item::Row(i) => {
                let row = &self.rows[i];
                let colors = cell_colors(row.look, selected, false, &theme);
                let dot = colors.dot.map(|color| {
                    let d = div().flex_none().size(px(8.0 * m.ui_scale)).rounded_full();
                    match row.look.dot {
                        DotStyle::Ring(_) => d.border(px(1.5 * m.ui_scale)).border_color(color),
                        _ => d.bg(color),
                    }
                });
                let name = format!(
                    "{}, {}, {}",
                    row.id,
                    relation_word(row.relation),
                    row.status
                );
                let mut label = div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(if selected {
                        theme.text_selected_primary
                    } else {
                        theme.text_primary
                    })
                    .child(row.id.clone());
                *label.text_style() = t_base(&m);
                let mut badge = div().flex_none().child(TlBadge::new(
                    TlBadgeKind::Neutral,
                    SharedString::new_static(relation_word(row.relation)),
                ));
                *badge.text_style() = t_xs(&m);
                let mut d = h_flex()
                    .id(("svc-dep-row", ix))
                    .debug_selector(move || format!("ServiceDeps-row-{ix}"))
                    .flex_none()
                    .w_full()
                    .h(m.row_height)
                    .px(m.sp[4])
                    .gap(m.sp[3])
                    .items_center()
                    .hover(|s| s.bg(theme.row_hover))
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(name))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        this.on_click(ix, ev, window, cx)
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_value_menu(ix, ev.position, window, cx);
                        }),
                    )
                    .children(dot)
                    .child(label)
                    .child(badge);
                if selected {
                    d = d.bg(theme.row_selected);
                }
                d.into_any_element()
            }
        }
    }

    fn build_list_items(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let end = range.end.min(self.items.len());
        (range.start..end)
            .map(|ix| self.item_element(ix, false, cx))
            .collect()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u32 {
        self.renders
    }

    /// The tab's lines: a heading, "(None)", "Ordering (N)" as shown, a row as
    /// "<unit> <Relation>". "Collecting…" before the details arrive.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn lines_for_test(&self) -> Vec<String> {
        if self.details.is_none() {
            return vec![self.collecting.to_string()];
        }
        self.items
            .iter()
            .map(|item| match *item {
                Item::Heading(Block::DependsOn) => self.heading_depends.to_string(),
                Item::Heading(_) => self.heading_dependents.to_string(),
                Item::Empty => self.none.to_string(),
                Item::OrderingHeader => self.ordering.to_string(),
                Item::Row(i) => {
                    format!(
                        "{} {}",
                        self.rows[i].id,
                        relation_word(self.rows[i].relation)
                    )
                }
            })
            .collect()
    }

    /// The accessible name and look of each shown row, in order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rows_for_test(&self) -> Vec<(SharedString, StatusLook)> {
        self.items
            .iter()
            .filter_map(|item| match *item {
                Item::Row(i) => {
                    let r = &self.rows[i];
                    Some((
                        SharedString::from(format!(
                            "{}, {}, {}",
                            r.id,
                            relation_word(r.relation),
                            r.status
                        )),
                        r.look,
                    ))
                }
                _ => None,
            })
            .collect()
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn focus_handle_for_test(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl ServicePropsTabView for DependenciesTab {
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    fn apply(&mut self, _window: &mut Window, _cx: &mut App) -> SmallVec<[ActionId; 2]> {
        SmallVec::new()
    }

    fn reset(&mut self, _cx: &mut App) {}

    fn set_details(&mut self, details: Option<&Arc<UnitDetails>>, _not_loaded: bool, cx: &mut App) {
        let same = match (&self.details, details) {
            (Some(have), Some(new)) => Arc::ptr_eq(have, new),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.details = details.cloned();
        self.rebuild(cx);
        cx.notify(self.entity_id);
    }
}

impl Render for DependenciesTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let body: AnyElement = if self.details.is_none() {
            let mut d = div()
                .px(m.sp[4])
                .py(m.sp[3])
                .text_color(theme.text_secondary)
                .child(self.collecting.clone());
            *d.text_style() = t_base(&m);
            d.into_any_element()
        } else if self.counts.iter().any(|&n| n > LIST_ROWS) {
            uniform_list(
                "svc-deps-list",
                self.items.len(),
                cx.processor(Self::build_list_items),
            )
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        } else {
            let items: Vec<AnyElement> = (0..self.items.len())
                .map(|ix| self.item_element(ix, true, cx))
                .collect();
            div()
                .size_full()
                .overflow_y_scrollbar()
                .child(v_flex().w_full().pb(m.sp[4]).children(items))
                .into_any_element()
        };
        v_flex()
            .id("ServiceDepsTab")
            .debug_selector(|| "ServiceDepsTab".into())
            .track_focus(&self.focus)
            .size_full()
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_copy_value))
            .child(body)
    }
}
