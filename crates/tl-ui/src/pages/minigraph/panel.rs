//! `MinigraphPanel`: the minigraph cards over the Processes table (E26-004, PERF-080, PERF-082,
//! APP-021, R-089, UI-PRF-040, ADR-011).
//!
//! The panel owns the cards and places them in the 4 × 3 slot grid from the bottom right. It
//! follows the `graphs.minigraph_*` settings, writes `[[minigraphs]]` on every change of the
//! card set (tl-app debounces the write by 500 ms), handles `ResetMinigraphLayout`,
//! `MinigraphHide` and `MinigraphRemove`, and sends the cards' demand as one surface while the
//! Processes tab is shown (as the process table does, pdu.52). The cards' `graph_targets` also go
//! out as an always-on part, so process, name and app cards keep sampling with the tab inactive
//! or the window hidden (PERF-085, PROC-093). It re-renders only when the card set or the drag
//! preview changes; each card is a cached view that re-renders on its own.
//!
//! Dragging a card (UI-PRF-043, PERF-082) previews the new order while the pointer is over a slot
//! and moves the card there on drop, with no holes.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, Context, DragMoveEvent, ElementId,
    Entity, Global, InteractiveElement as _, IntoElement, MouseButton, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, StyleRefinement, Styled as _, Subscription,
    WeakEntity, Window, canvas, div, px,
};
use tl_model::demand::{CpuDemand, Demand, GRAPH_TARGETS_MAX};
use tl_model::settings::GraphsSettings;

use super::GraphColor;
use super::actions::{
    MinigraphHide, MinigraphMoveToSlot, MinigraphRemove, MinigraphSetColor, MinigraphSetRange,
    ResetMinigraphLayout,
};
use super::card::{
    CardEnded, DraggedMinigraph, MINI_CARD_H, MINI_CARD_W, MinigraphCard, past_threshold,
};
use super::sources::{
    COLS, GraphColumn, MAX_SLOTS, MinigraphEntry, MinigraphId, MinigraphSource, ROWS,
    default_entries, from_state, slot_cell, to_state,
};
use crate::components::motion::{M_FAST, ease_out_cubic};
use crate::components::toast::ToastKind;
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::pages::performance::{DeviceKey, DeviceKind};
use crate::settings_global::{SettingValue, TlSettings, animations_on};
use crate::theme::metrics;
use crate::window::main::push_main_toast;

/// Space between the grid and the table's bottom and right edges, before `ui_scale`.
const MARGIN: f32 = 16.;
/// Space between two cards, before `ui_scale`.
const GAP: f32 = 8.;
/// The range of a card added from View > Graphs (PERF-081).
const NEW_RANGE_S: u16 = 60;

/// All 12 slots hold an attached card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelFull;

/// The seven system kinds in default slot order, each with its column.
const SYSTEM: [(MinigraphSource, GraphColumn); 7] = [
    (MinigraphSource::Cpu, GraphColumn::Cpu),
    (MinigraphSource::Memory, GraphColumn::Memory),
    (MinigraphSource::Disk, GraphColumn::Disk),
    (MinigraphSource::Network, GraphColumn::Network),
    (MinigraphSource::Gpu(None), GraphColumn::Gpu),
    (MinigraphSource::Vram(None), GraphColumn::Vram),
    (MinigraphSource::GpuTemp(None), GraphColumn::Gpu),
];

/// A card drag past the 4 px threshold (UI-PRF-043).
struct DragPreview {
    id: MinigraphId,
    /// The slot under the pointer; `None` in a gap or off the grid, which shows the old order.
    over: Option<u16>,
    /// `over` before its last change; the shift animates from that order.
    from: Option<u16>,
    /// Counts the changes of `over`, so each shift starts a new animation.
    epoch: usize,
}

/// The app's panel, set by `MinigraphPanel::new` (ADR-009: one panel per app). The shell menu
/// and the row menu reach the panel through it without the Processes page.
pub struct GlobalMinigraphPanel(pub WeakEntity<MinigraphPanel>);

impl Global for GlobalMinigraphPanel {}

pub struct MinigraphPanel {
    /// Attached cards, sorted by slot.
    cards: Vec<(MinigraphId, Entity<MinigraphCard>)>,
    /// Every entry, attached and detached (E26-015 uses the detached ones).
    entries: Vec<MinigraphEntry>,
    next_id: u32,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    /// The Processes tab is shown; only then is the demand part sent (ADR-008).
    active: bool,
    /// The `graphs` section the panel last followed.
    graphs: GraphsSettings,
    drag: Option<DragPreview>,
    /// `ui_scale` of the last render, for `slot_at`.
    scale: f32,
    /// The panel bounds of the last paint, for the slot under a release.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    _settings_sub: Option<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    pub render_count: std::cell::Cell<u32>,
    /// Every toast the panel pushed, in order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub toasts: Vec<(ToastKind, gpui_kit::SharedString)>,
}

impl MinigraphPanel {
    /// Loads `UiState.minigraphs` (the defaults when empty), drops the system cards whose
    /// setting is off, adds the ones whose setting is on and are missing, and creates the
    /// attached cards.
    pub fn new(
        hub: &Entity<SnapshotHub>,
        demand: &Entity<DemandAggregator>,
        cx: &mut Context<Self>,
    ) -> Self {
        let has_settings = cx.has_global::<TlSettings>();
        let (state, graphs) = if has_settings {
            (
                TlSettings::state(cx).minigraphs.clone(),
                TlSettings::settings(cx).graphs.clone(),
            )
        } else {
            (Vec::new(), GraphsSettings::default())
        };
        let mut next_id = 0;
        let entries = if state.is_empty() {
            let entries = default_entries();
            next_id = entries.len() as u32;
            entries
        } else {
            from_state(&state, &mut next_id)
        };
        let _settings_sub =
            has_settings.then(|| cx.observe_global::<TlSettings>(Self::on_settings));
        register_actions(cx);
        let weak = cx.weak_entity();
        cx.set_global(GlobalMinigraphPanel(weak));
        let mut this = Self {
            cards: Vec::new(),
            entries,
            next_id,
            hub: hub.clone(),
            demand: demand.clone(),
            active: false,
            graphs: graphs.clone(),
            drag: None,
            scale: 1.,
            bounds: Rc::default(),
            _settings_sub,
            #[cfg(any(test, feature = "test-hooks"))]
            render_count: std::cell::Cell::new(0),
            #[cfg(any(test, feature = "test-hooks"))]
            toasts: Vec::new(),
        };
        this.entries
            .retain(|e| system_on(&graphs, &e.source).unwrap_or(true));
        for (source, column) in SYSTEM {
            if system_on(&graphs, &source) == Some(true) && !this.has_system(&source) {
                // Full: the card stays off until a slot frees up; no toast at startup.
                let _ = this.insert(source, column);
            }
        }
        this.rebuild_cards(cx);
        this.send_graph_targets(cx);
        let source = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.demand.update(cx, |a, cx| {
                a.remove(source, cx);
                a.set_graph_targets(source, Arc::from([]), cx);
            });
        })
        .detach();
        this
    }

    /// The app's panel, `None` before the Processes page is built.
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalMinigraphPanel>()?.0.upgrade()
    }

    pub fn attached(&self) -> impl Iterator<Item = &MinigraphEntry> {
        self.entries.iter().filter(|e| !e.detached)
    }

    /// Every entry, attached and detached.
    pub fn entries(&self) -> &[MinigraphEntry] {
        &self.entries
    }

    /// The attached cards, sorted by slot.
    pub fn cards(&self) -> &[(MinigraphId, Entity<MinigraphCard>)] {
        &self.cards
    }

    /// The card being dragged past the 4 px threshold.
    pub fn dragging(&self) -> Option<MinigraphId> {
        self.drag.as_ref().map(|d| d.id)
    }

    /// The slot each attached card is drawn at: the drag preview order while a dragged card is
    /// over a slot, else the stored slots.
    pub fn preview_slots(&self, cx: &App) -> Vec<(MinigraphId, u16)> {
        let over = self.drag.as_ref().map(|d| (d.id, d.over));
        let slots = self.arranged(over, cx);
        self.cards.iter().map(|(id, _)| *id).zip(slots).collect()
    }

    /// The slot under `p`, from the 4 × 3 slot rects laid out from the bottom right of `bounds`.
    /// A gap or a point off the grid gives `None`; an empty slot counts.
    pub fn slot_at(&self, p: Point<Pixels>, bounds: Bounds<Pixels>) -> Option<u16> {
        let s = self.scale;
        let x = f32::from(bounds.right() - p.x) / s - MARGIN;
        let y = f32::from(bounds.bottom() - p.y) / s - MARGIN;
        let cell = |d: f32, size: f32, count: u16| {
            if d <= 0. {
                return None;
            }
            let i = (d / (size + GAP)).floor();
            let inside = d - i * (size + GAP);
            (i < f32::from(count) && inside > 0. && inside <= size).then_some(i as u16)
        };
        let col = cell(x, MINI_CARD_W, COLS)?;
        let row = cell(y, MINI_CARD_H, ROWS)?;
        Some(row * COLS + col)
    }

    /// `MinigraphMoveToSlot`: the card takes `slot` and the others shift to fill. The attached
    /// cards then sit at slots 0 to n - 1 in that order (no holes), so a slot past the last card
    /// is the next free slot.
    pub fn move_to_slot(&mut self, id: MinigraphId, slot: u16, cx: &mut Context<Self>) {
        let mut order: Vec<(u16, MinigraphId)> = self.attached().map(|e| (e.slot, e.id)).collect();
        order.sort_unstable_by_key(|(slot, _)| *slot);
        let mut order: Vec<MinigraphId> = order.into_iter().map(|(_, id)| id).collect();
        let Some(from) = order.iter().position(|o| *o == id) else {
            return;
        };
        move_within(&mut order, from, slot);
        let mut moved = false;
        for (id, slot) in order.into_iter().zip(0u16..) {
            if let Some(e) = self.entries.iter_mut().find(|e| e.id == id && !e.detached) {
                moved |= e.slot != slot;
                e.slot = slot;
            }
        }
        if moved {
            self.changed(cx);
        }
    }

    /// The slot of each card in `cards` order, with `dragged` moved to its slot under the
    /// pointer. With no slot under the pointer the stored slots stay.
    fn arranged(&self, dragged: Option<(MinigraphId, Option<u16>)>, cx: &App) -> Vec<u16> {
        let from = dragged
            .and_then(|(id, over)| Some((self.cards.iter().position(|(c, _)| *c == id)?, over?)));
        let Some((from, over)) = from else {
            return self
                .cards
                .iter()
                .map(|(_, c)| c.read(cx).entry.slot)
                .collect();
        };
        let mut order: Vec<usize> = (0..self.cards.len()).collect();
        move_within(&mut order, from, over);
        let mut slots = vec![0; order.len()];
        for (i, slot) in order.into_iter().zip(0u16..) {
            slots[i] = slot;
        }
        slots
    }

    /// Starts the preview of a drag of `id` once `pos` is 4 px from the press on the card.
    fn start_drag(&mut self, id: MinigraphId, pos: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some((_, card)) = self.cards.iter().find(|(c, _)| *c == id) else {
            return false;
        };
        if !card
            .read(cx)
            .press()
            .is_some_and(|p| past_threshold(p, pos))
        {
            return false;
        }
        card.update(cx, |c, cx| c.set_dragging(true, cx));
        self.drag = Some(DragPreview {
            id,
            over: None,
            from: None,
            epoch: 0,
        });
        true
    }

    /// A drag move of a card: starts the preview past 4 px and follows the slot under the
    /// pointer. Re-renders only when that slot changes.
    fn on_drag_move(&mut self, ev: &DragMoveEvent<DraggedMinigraph>, cx: &mut Context<Self>) {
        let pos = ev.event.position;
        if self.drag.is_none() && !self.start_drag(ev.drag(cx).id, pos, cx) {
            return;
        }
        let over = self.slot_at(pos, ev.bounds);
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if drag.over != over {
            drag.from = drag.over;
            drag.over = over;
            drag.epoch = drag.epoch.wrapping_add(1);
            cx.notify();
        }
    }

    /// A left release. After a drag, a release at `at` over a slot moves the card there;
    /// anywhere else the cards go back to their slots.
    fn end_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        // GPUI starts its drag on a move after the panel's drag-move listener ran, so a release
        // right after one move past 4 px finds no preview yet.
        if self.drag.is_none() {
            let held = self
                .cards
                .iter()
                .find(|(_, c)| c.read(cx).gpui_drag())
                .map(|(id, _)| *id);
            if let Some(id) = held {
                self.start_drag(id, at, cx);
            }
        }
        let drag = self.drag.take();
        for (_, card) in &self.cards {
            card.update(cx, |c, cx| c.end_drag(cx));
        }
        let Some(drag) = drag else {
            return;
        };
        if let Some(slot) = self.slot_at(at, self.bounds.get()) {
            self.move_to_slot(drag.id, slot, cx);
        }
        cx.notify();
    }

    /// Adds a system card at the next free slot. `Err` when 12 cards are attached; the caller
    /// shows the `minigraph-toast-full` toast.
    pub fn add_system(
        &mut self,
        source: MinigraphSource,
        cx: &mut Context<Self>,
    ) -> Result<MinigraphId, PanelFull> {
        let column = SYSTEM
            .iter()
            .find(|(s, _)| same_kind(s, &source))
            .map_or(GraphColumn::Cpu, |(_, c)| *c);
        let id = self.insert(source, column)?;
        self.changed(cx);
        Ok(id)
    }

    /// Hides a card. A system card's setting turns off, so View > Graphs unchecks; any other
    /// card is removed.
    pub fn hide(&mut self, id: MinigraphId, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter().find(|e| e.id == id) else {
            return;
        };
        let Some(key) = entry.source.settings_key() else {
            self.remove(id, cx);
            return;
        };
        if let Some(flag) = flag_mut(&mut self.graphs, &entry.source) {
            *flag = false;
        }
        self.entries.retain(|e| e.id != id);
        self.changed(cx);
        // The observer sees `graphs` already equal to the new value and does nothing.
        if cx.has_global::<TlSettings>() {
            TlSettings::set(cx, key, SettingValue::Bool(false));
        }
    }

    pub fn remove(&mut self, id: MinigraphId, cx: &mut Context<Self>) {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        if self.entries.len() != before {
            self.changed(cx);
        }
    }

    /// `MinigraphSetRange` (PERF-081): the card's chart range and its `[[minigraphs]]` entry.
    pub fn set_range(&mut self, id: MinigraphId, secs: u16, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) else {
            return;
        };
        if entry.range_s == secs {
            return;
        }
        entry.range_s = secs;
        if let Some((_, card)) = self.cards.iter().find(|(c, _)| *c == id) {
            card.update(cx, |c, cx| c.set_range(secs, cx));
        }
        self.persist(cx);
    }

    /// `MinigraphSetColor` (PERF-081): the card's line colour and its `[[minigraphs]]` entry.
    pub fn set_color(&mut self, id: MinigraphId, color: GraphColor, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) else {
            return;
        };
        if entry.color == color {
            return;
        }
        entry.color = color;
        if let Some((_, card)) = self.cards.iter().find(|(c, _)| *c == id) {
            card.update(cx, |c, cx| c.set_color(color, cx));
        }
        self.persist(cx);
    }

    /// `ResetMinigraphLayout` (UI-PRF-044): the six default cards at slots 0 to 5 with the
    /// default range and colour, GPU temp at slot 6 when its setting is on. Detached cards are
    /// attached again (E26-015 closes their windows) and process cards are removed. The six
    /// default settings turn on.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        let mut entries = default_entries();
        for e in &mut entries {
            e.id = self.take_id();
        }
        self.entries = entries;
        for (source, _) in &SYSTEM[..6] {
            if let Some(flag) = flag_mut(&mut self.graphs, source) {
                *flag = true;
            }
        }
        if self.graphs.minigraph_gpu_temperature {
            let _ = self.insert(MinigraphSource::GpuTemp(None), GraphColumn::Gpu);
        }
        let graphs = self.graphs.clone();
        if cx.has_global::<TlSettings>() {
            TlSettings::update(cx, |d| {
                d.settings.graphs = GraphsSettings {
                    keep_detached_on_close: d.settings.graphs.keep_detached_on_close,
                    ..graphs
                };
            });
        }
        self.changed(cx);
        self.toast(ToastKind::Success, "minigraph-toast-reset", cx);
    }

    /// Writes `UiState.minigraphs`; tl-app debounces the file write by 500 ms (ADR-011).
    pub fn persist(&self, cx: &mut App) {
        if !cx.has_global::<TlSettings>() {
            return;
        }
        let state = to_state(&self.entries);
        TlSettings::update(cx, |d| d.state.minigraphs = state);
    }

    /// Called by the Processes page on tab activation. Active sends the demand part; inactive
    /// withdraws it. The always-on graph targets stay.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else {
            let source = cx.entity_id();
            self.demand.update(cx, |a, cx| a.remove(source, cx));
        }
    }

    /// The sum of the attached cards' parts. `cpu_fast` is never set (UI-PRF-040).
    /// `graph_targets` is the union of the cards' identities in slot order, at most
    /// `GRAPH_TARGETS_MAX` (PERF-085, R-154).
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut d = Demand::default();
        let mut targets = Vec::new();
        for (_, card) in &self.cards {
            let p = card.read(cx).demand_part();
            for t in p.graph_targets.iter() {
                if targets.len() < GRAPH_TARGETS_MAX && !targets.contains(t) {
                    targets.push(t.clone());
                }
            }
            d.any_surface_visible |= p.any_surface_visible;
            if p.cpu != CpuDemand::Off {
                d.cpu = CpuDemand::Total;
            }
            d.memory |= p.memory;
            d.disks |= p.disks;
            d.nets |= p.nets;
            d.gpus |= p.gpus;
        }
        d.graph_targets = targets.into();
        d
    }

    fn send_demand(&self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        let part = self.demand_part(cx);
        let source = cx.entity_id();
        self.demand.update(cx, |a, cx| {
            let window = a.main_window();
            a.set(source, window, part, cx);
        });
    }

    /// Sends the cards' `graph_targets` as the always-on part, whether the tab is active or not.
    fn send_graph_targets(&self, cx: &mut Context<Self>) {
        let targets = self.demand_part(cx).graph_targets;
        let source = cx.entity_id();
        self.demand
            .update(cx, |a, cx| a.set_graph_targets(source, targets, cx));
    }

    /// The card set changed: rebuild the cards, resend demand, persist, redraw.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.rebuild_cards(cx);
        self.send_graph_targets(cx);
        self.send_demand(cx);
        self.persist(cx);
        cx.notify();
    }

    /// Keeps the cards of attached entries that still exist and creates the missing ones.
    fn rebuild_cards(&mut self, cx: &mut Context<Self>) {
        let mut attached: Vec<MinigraphEntry> = self.attached().cloned().collect();
        attached.sort_by_key(|e| e.slot);
        let catalog = Arc::clone(&self.hub.read(cx).current().devices);
        let order = gpu_order(cx);
        let mut old = std::mem::take(&mut self.cards);
        let mut cards = Vec::with_capacity(attached.len());
        for entry in attached {
            let card = match old.iter().position(|(id, _)| *id == entry.id) {
                Some(i) => old.swap_remove(i).1,
                None => {
                    let (entry, hub) = (entry.clone(), self.hub.clone());
                    let card = cx.new(|cx| MinigraphCard::new(entry, &catalog, &order, &hub, cx));
                    // An ended card drops its graph target (PERF-086).
                    cx.subscribe(&card, |this, _, _: &CardEnded, cx| {
                        this.send_graph_targets(cx);
                        this.send_demand(cx);
                    })
                    .detach();
                    card
                }
            };
            card.update(cx, |c, _| c.entry.slot = entry.slot);
            cards.push((entry.id, card));
        }
        self.cards = cards;
    }

    /// Adds an entry at the next free slot.
    fn insert(
        &mut self,
        source: MinigraphSource,
        column: GraphColumn,
    ) -> Result<MinigraphId, PanelFull> {
        let slot = (0..MAX_SLOTS)
            .find(|s| !self.attached().any(|e| e.slot == *s))
            .ok_or(PanelFull)?;
        let id = self.take_id();
        self.entries.push(MinigraphEntry {
            id,
            source,
            column,
            range_s: NEW_RANGE_S,
            color: GraphColor::Accent,
            slot,
            detached: false,
            window: 0,
        });
        Ok(id)
    }

    fn take_id(&mut self) -> MinigraphId {
        let id = MinigraphId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn has_system(&self, source: &MinigraphSource) -> bool {
        self.entries.iter().any(|e| same_kind(&e.source, source))
    }

    /// Follows View > Graphs: a flag turning on adds its card, turning off removes it.
    fn on_settings(&mut self, cx: &mut Context<Self>) {
        let graphs = TlSettings::settings(cx).graphs.clone();
        if graphs == self.graphs {
            return;
        }
        let old = std::mem::replace(&mut self.graphs, graphs.clone());
        let mut changed = false;
        for (source, column) in SYSTEM {
            let (was, now) = (system_on(&old, &source), system_on(&graphs, &source));
            if was == now {
                continue;
            }
            if now == Some(true) {
                if self.has_system(&source) {
                    continue;
                }
                match self.insert(source, column) {
                    Ok(_) => changed = true,
                    Err(PanelFull) => self.toast(ToastKind::Warning, "minigraph-toast-full", cx),
                }
            } else {
                let before = self.entries.len();
                self.entries.retain(|e| !same_kind(&e.source, &source));
                changed |= self.entries.len() != before;
            }
        }
        if changed {
            self.changed(cx);
        }
    }

    fn toast(&mut self, kind: ToastKind, key: &'static str, cx: &mut Context<Self>) {
        let text = tr(cx, key);
        #[cfg(any(test, feature = "test-hooks"))]
        self.toasts.push((kind, text.clone()));
        cx.defer(move |cx| push_main_toast(cx, kind, text, None, None));
    }
}

/// Toasts `minigraph-toast-full` for an `add_system` that returned `PanelFull`.
pub fn toast_full(panel: &Entity<MinigraphPanel>, cx: &mut App) {
    panel.update(cx, |p, cx| {
        p.toast(ToastKind::Warning, "minigraph-toast-full", cx)
    });
}

/// The minigraph actions reach the panel from the shell menu and the card menus through the
/// window's dispatch, whatever has focus. `App::on_action` listens in the bubble phase, after
/// the panel root's own listeners.
fn register_actions(cx: &mut Context<MinigraphPanel>) {
    let weak = cx.weak_entity();
    let app: &mut App = cx;
    let panel = weak.clone();
    app.on_action(move |_: &ResetMinigraphLayout, cx| {
        let _ = panel.update(cx, |p, cx| p.reset(cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphHide, cx| {
        let _ = panel.update(cx, |p, cx| p.hide(a.id, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphRemove, cx| {
        let _ = panel.update(cx, |p, cx| p.remove(a.id, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphMoveToSlot, cx| {
        let _ = panel.update(cx, |p, cx| p.move_to_slot(a.id, a.slot, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphSetColor, cx| {
        let _ = panel.update(cx, |p, cx| p.set_color(a.id, a.color, cx));
    });
    app.on_action(move |a: &MinigraphSetRange, cx| {
        let _ = weak.update(cx, |p, cx| p.set_range(a.id, a.secs, cx));
    });
}

/// Moves `v[from]` to index `to`, or to the end when `to` is past it.
fn move_within<T>(v: &mut Vec<T>, from: usize, to: u16) {
    let item = v.remove(from);
    let to = usize::from(to).min(v.len());
    v.insert(to, item);
}

/// Same system kind; a GPU key does not count.
fn same_kind(a: &MinigraphSource, b: &MinigraphSource) -> bool {
    a.is_system() && std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// The setting of a system source; `None` for process, name and app cards.
fn system_on(graphs: &GraphsSettings, source: &MinigraphSource) -> Option<bool> {
    let mut graphs = graphs.clone();
    flag_mut(&mut graphs, source).map(|f| *f)
}

fn flag_mut<'a>(graphs: &'a mut GraphsSettings, source: &MinigraphSource) -> Option<&'a mut bool> {
    Some(match source {
        MinigraphSource::Cpu => &mut graphs.minigraph_cpu,
        MinigraphSource::Memory => &mut graphs.minigraph_memory,
        MinigraphSource::Disk => &mut graphs.minigraph_disk,
        MinigraphSource::Network => &mut graphs.minigraph_network,
        MinigraphSource::Gpu(_) => &mut graphs.minigraph_gpu,
        MinigraphSource::Vram(_) => &mut graphs.minigraph_vram,
        MinigraphSource::GpuTemp(_) => &mut graphs.minigraph_gpu_temperature,
        MinigraphSource::Process { .. } | MinigraphSource::Name(_) | MinigraphSource::App(_) => {
            return None;
        }
    })
}

/// The GPU keys of `performance.device_order`, for a card with no GPU key (R-085).
pub(crate) fn gpu_order(cx: &App) -> Vec<DeviceKey> {
    if !cx.has_global::<TlSettings>() {
        return Vec::new();
    }
    TlSettings::settings(cx)
        .performance
        .device_order
        .iter()
        .map(|k| DeviceKey(Arc::from(k.as_str())))
        .filter(|k| k.kind() == DeviceKind::Gpu)
        .collect()
}

impl Render for MinigraphPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        self.render_count.set(self.render_count.get() + 1);
        // The host mounts the panel cached with an `absolute().inset_0()` style over the
        // Processes table; a cached view lays out as its own root, so the root fills that box.
        // The drag and drop handlers make the root a normal hitbox: it does not block the
        // table, and only the cards take clicks.
        let mut root = div()
            .id("MinigraphPanel")
            .debug_selector(|| "MinigraphPanel".into())
            .relative()
            .size_full()
            .on_action(cx.listener(|this, _: &ResetMinigraphLayout, _, cx| this.reset(cx)))
            .on_action(cx.listener(|this, a: &MinigraphHide, _, cx| this.hide(a.id, cx)))
            .on_action(cx.listener(|this, a: &MinigraphRemove, _, cx| this.remove(a.id, cx)))
            .on_action(
                cx.listener(|this, a: &MinigraphSetRange, _, cx| this.set_range(a.id, a.secs, cx)),
            )
            .on_action(
                cx.listener(|this, a: &MinigraphSetColor, _, cx| this.set_color(a.id, a.color, cx)),
            )
            .on_action(cx.listener(|this, a: &MinigraphMoveToSlot, _, cx| {
                this.move_to_slot(a.id, a.slot, cx)
            }))
            .on_drag_move(
                cx.listener(|this, ev: &DragMoveEvent<DraggedMinigraph>, _, cx| {
                    this.on_drag_move(ev, cx)
                }),
            )
            .on_drop(cx.listener(|this, _: &DraggedMinigraph, window, cx| {
                this.end_drag(window.mouse_position(), cx)
            }))
            // GPUI drops the drag on a release off the panel and fires no drop. A card's blocking
            // label row also hides the panel from the pointer, so the slot still decides.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| this.end_drag(ev.position, cx)),
            );
        let bounds = Rc::clone(&self.bounds);
        root = root.child(
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .size_full(),
        );
        let s = metrics(cx).ui_scale;
        self.scale = s;
        if self.cards.is_empty() {
            return root;
        }
        let (w, h) = (MINI_CARD_W * s, MINI_CARD_H * s);
        let offset = |slot: u16| {
            let (row, col) = slot_cell(slot);
            (
                (MARGIN + f32::from(col) * (MINI_CARD_W + GAP)) * s,
                (MARGIN + f32::from(row) * (MINI_CARD_H + GAP)) * s,
            )
        };
        let drag = self.drag.as_ref().map(|d| (d.id, d.over, d.from, d.epoch));
        let now = self.arranged(drag.map(|(id, over, _, _)| (id, over)), cx);
        // The order before the last change of the slot under the pointer, to slide from.
        let before = match drag {
            Some((id, _, from, _)) if animations_on(cx) => {
                Some(self.arranged(Some((id, from)), cx))
            }
            _ => None,
        };
        // The dragged card shows its home outline at its stored slot, under the card that
        // shifts there, so it comes first.
        let dragged = drag.and_then(|(id, ..)| self.cards.iter().position(|(c, _)| *c == id));
        let order = dragged
            .into_iter()
            .chain((0..self.cards.len()).filter(|i| Some(*i) != dragged));
        for i in order {
            let (id, card) = &self.cards[i];
            let slot = if Some(i) == dragged {
                card.read(cx).entry.slot
            } else {
                now[i]
            };
            let (right, bottom) = offset(slot);
            let el = div()
                .id(("MinigraphSlot", u64::from(id.0)))
                .debug_selector(move || format!("MinigraphSlot-{slot}"))
                .absolute()
                .right(px(right))
                .bottom(px(bottom))
                .w(px(w))
                .h(px(h))
                .child(
                    card.clone()
                        .cached(StyleRefinement::default().w(px(w)).h(px(h))),
                );
            let old = before
                .as_ref()
                .filter(|_| Some(i) != dragged)
                .map(|b| b[i])
                .filter(|old| *old != slot);
            match (old, drag) {
                (Some(old), Some((_, _, _, epoch))) => {
                    let (r0, b0) = offset(old);
                    let id = ElementId::NamedInteger(
                        format!("MinigraphShift-{}", id.0).into(),
                        epoch as u64,
                    );
                    root = root.child(el.with_animation(
                        id,
                        Animation::new(M_FAST).with_easing(ease_out_cubic),
                        move |el, t| {
                            el.right(px(r0 + (right - r0) * t))
                                .bottom(px(b0 + (bottom - b0) * t))
                        },
                    ));
                }
                _ => root = root.child(el),
            }
        }
        root
    }
}
