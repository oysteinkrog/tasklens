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
//! and moves the card there on drop, with no holes. A release more than 24 px outside the main
//! window detaches the card instead. Detaching and attaching (E26-015, R-084) keep the detached
//! graph windows and the `[[detached_windows]]` list in step with the detached entries.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, Context, DragMoveEvent, ElementId,
    Entity, Global, InteractiveElement as _, IntoElement, MouseButton, MouseUpEvent,
    ParentElement as _, Pixels, Point, Render, SharedString, Size, StyleRefinement, Styled as _,
    Subscription, WeakEntity, Window, canvas, div, px,
};
use tl_model::demand::{CpuDemand, Demand, GRAPH_TARGETS_MAX};
use tl_model::process::ProcFlags;
use tl_model::settings::{DetachedWindowState, GraphsSettings};

use super::GraphColor;
use super::actions::{
    AddRandomMinigraphs, MinigraphAddToWindow, MinigraphAttach, MinigraphDetach, MinigraphHide,
    MinigraphMoveToSlot, MinigraphRemove, MinigraphSetColor, MinigraphSetRange,
    ResetMinigraphLayout,
};
use super::card::{
    CardEnded, DraggedMinigraph, MINI_CARD_H, MINI_CARD_W, MinigraphCard, past_threshold,
};
use super::sources::{
    COLS, GraphColumn, MAX_SLOTS, MinigraphEntry, MinigraphId, MinigraphSource, ROWS,
    compact_slots, default_entries, from_state, slot_cell, to_state,
};
use crate::components::motion::{M_FAST, ease_out_cubic};
use crate::components::toast::ToastKind;
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::{tr, tr_args};
use crate::pages::performance::{DeviceKey, DeviceKind};
use crate::settings_global::{SettingValue, TlSettings, animations_on};
use crate::theme::metrics;
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, WindowKey};
use crate::windows::detached_graph::{DetachedGraphWindow, open_detached, window_size};

/// Space between the grid and the table's bottom and right edges, before `ui_scale`.
const MARGIN: f32 = 16.;
/// Space between two cards, before `ui_scale`.
const GAP: f32 = 8.;
/// The range of a card added from View > Graphs (PERF-081).
const NEW_RANGE_S: u16 = 60;
/// A release this far outside the main window detaches the dragged card (UI-PRF-043).
const DETACH_EDGE: f32 = 24.;

/// All 12 slots hold an attached card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanelFull;

/// Why `add_process` added no card (UI-PRC-020 items 15 to 17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddError {
    /// A card with this source and column exists, attached or detached.
    Exists,
    /// All 12 slots hold an attached card.
    Full,
    /// The row's identity could not be resolved to a source.
    Unavailable,
}

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
    /// Every entry, attached and detached.
    entries: Vec<MinigraphEntry>,
    /// The `[[detached_windows]]` entries: one per window that holds cards.
    windows: Vec<DetachedWindowState>,
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
        let (state, windows, graphs) = if has_settings {
            (
                TlSettings::state(cx).minigraphs.clone(),
                TlSettings::state(cx).detached_windows.clone(),
                TlSettings::settings(cx).graphs.clone(),
            )
        } else {
            (Vec::new(), Vec::new(), GraphsSettings::default())
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
            windows,
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
        this.sync_windows(cx);
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

    /// The pointer at `pos` in a drag of `id`: starts the preview past 4 px and follows the slot
    /// under the pointer. Re-renders when the preview starts and when that slot changes. The
    /// card calls it when GPUI starts the drag, the drag-move listener on every later move.
    pub fn drag_to(&mut self, id: MinigraphId, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let started = self.drag.is_none();
        if started && !self.start_drag(id, pos, cx) {
            return;
        }
        let over = self.slot_at(pos, self.bounds.get());
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if drag.over != over {
            drag.from = drag.over;
            drag.over = over;
            drag.epoch = drag.epoch.wrapping_add(1);
            cx.notify();
        } else if started {
            cx.notify();
        }
    }

    fn on_drag_move(&mut self, ev: &DragMoveEvent<DraggedMinigraph>, cx: &mut Context<Self>) {
        self.drag_to(ev.drag(cx).id, ev.event.position, cx);
    }

    /// A left release. After a drag, a release at `at` over a slot moves the card there;
    /// anywhere else the cards go back to their slots.
    fn end_drag(&mut self, at: Point<Pixels>, window_size: Size<Pixels>, cx: &mut Context<Self>) {
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
        if outside_window(at, window_size) {
            // The compositor places the new window; KDE placement is the KWin bridge's (E26-017).
            self.detach(drag.id, cx);
        } else if let Some(slot) = self.slot_at(at, self.bounds.get()) {
            self.move_to_slot(drag.id, slot, cx);
        }
        cx.notify();
    }

    /// `MinigraphDetach` (R-084): the attached card moves to the lowest unused window id from 1
    /// and the others close the gap.
    pub fn detach(&mut self, id: MinigraphId, cx: &mut Context<Self>) {
        let window = (1u32..)
            .find(|n| !self.entries.iter().any(|e| e.detached && e.window == *n))
            .unwrap_or(1);
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id && !e.detached) else {
            return;
        };
        entry.detached = true;
        entry.window = window;
        compact_slots(&mut self.entries);
        self.changed(cx);
    }

    /// `MinigraphAttach`: the detached card takes the next free slot. With 12 attached cards the
    /// `minigraph-toast-full` toast shows and nothing changes.
    pub fn attach(&mut self, id: MinigraphId, cx: &mut Context<Self>) {
        if !self.entries.iter().any(|e| e.id == id && e.detached) {
            return;
        }
        if !self.attach_quietly(id) {
            self.toast(ToastKind::Warning, "minigraph-toast-full", cx);
            return;
        }
        self.changed(cx);
    }

    /// Attaches a detached card at the next free slot; false when none is free.
    fn attach_quietly(&mut self, id: MinigraphId) -> bool {
        let Some(slot) = (0..MAX_SLOTS).find(|s| !self.attached().any(|e| e.slot == *s)) else {
            return false;
        };
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id && e.detached) else {
            return false;
        };
        entry.detached = false;
        entry.window = 0;
        entry.slot = slot;
        true
    }

    /// `MinigraphAddToWindow`: adds a system card for `source` to detached window `window`, unless
    /// that window already has one of that kind.
    pub fn add_to_window(&mut self, window: u32, source: MinigraphSource, cx: &mut Context<Self>) {
        if !source.is_system()
            || self
                .entries
                .iter()
                .any(|e| e.detached && e.window == window && same_kind(&e.source, &source))
        {
            return;
        }
        let column = SYSTEM
            .iter()
            .find(|(s, _)| same_kind(s, &source))
            .map_or(GraphColumn::Cpu, |(_, c)| *c);
        // The card shows only while its View > Graphs setting is on, as at startup.
        let key = source.settings_key();
        if let Some(flag) = flag_mut(&mut self.graphs, &source) {
            *flag = true;
        }
        let id = self.take_id();
        self.entries.push(MinigraphEntry {
            id,
            source,
            column,
            range_s: NEW_RANGE_S,
            color: GraphColor::Accent,
            slot: 0,
            detached: true,
            window,
        });
        self.changed(cx);
        if let Some(key) = key
            && cx.has_global::<TlSettings>()
        {
            TlSettings::set(cx, key, SettingValue::Bool(true));
        }
    }

    /// Brings the detached windows in line with the detached entries: opens a window for a group
    /// with none, refills and resizes an open one, closes one with no cards left, and keeps the
    /// `[[detached_windows]]` list to match. The windows change in a deferred step, because
    /// opening or closing one inside this update, or inside an action sent from that window,
    /// fails.
    fn sync_windows(&mut self, cx: &mut Context<Self>) {
        let mut groups: BTreeMap<u32, Vec<MinigraphEntry>> = BTreeMap::new();
        for e in self.entries.iter().filter(|e| e.detached) {
            groups.entry(e.window).or_default().push(e.clone());
        }
        let scale = metrics(cx).ui_scale;
        let gone: Vec<u32> = self
            .windows
            .iter()
            .map(|w| w.id)
            .filter(|id| !groups.contains_key(id))
            .collect();
        self.windows.retain(|w| groups.contains_key(&w.id));
        let mut work = Vec::with_capacity(groups.len());
        for (id, cards) in groups {
            let size = window_size(cards.len(), scale);
            let saved = self.windows.iter().find(|w| w.id == id).cloned();
            let (width, height) = (f32::from(size.width) as u32, f32::from(size.height) as u32);
            match self.windows.iter_mut().find(|w| w.id == id) {
                Some(w) => {
                    w.width = width;
                    w.height = height;
                }
                None => self.windows.push(DetachedWindowState {
                    id,
                    width,
                    height,
                    ..DetachedWindowState::default()
                }),
            }
            work.push((id, cards, saved, size));
        }
        if work.is_empty() && gone.is_empty() {
            return;
        }
        let panel = cx.weak_entity();
        cx.defer(move |cx| {
            let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
                return;
            };
            let key = |id: u32| u8::try_from(id).ok().map(WindowKey::DetachedGraph);
            for id in gone {
                if let Some(key) = key(id) {
                    registry.update(cx, |r, cx| r.close(&key, cx));
                }
            }
            for (id, cards, saved, size) in work {
                let open = key(id).and_then(|k| registry.read(cx).get(&k));
                if let Some(handle) = open {
                    if let Some(view) = DetachedGraphWindow::find(handle, cx) {
                        view.update(cx, |v, cx| v.set_cards(cards, cx));
                    }
                    let _ = handle.update(cx, |_, window, _| window.resize(size));
                } else if open_detached(id, cards.clone(), saved.as_ref(), cx).is_err() {
                    // open_detached toasted. Put the cards back so none is lost.
                    let ids: Vec<MinigraphId> = cards.iter().map(|e| e.id).collect();
                    let _ = panel.update(cx, |p, cx| p.reattach_all(&ids, cx));
                }
            }
        });
    }

    /// Attaches `ids` after their window failed to open, as many as there are free slots.
    fn reattach_all(&mut self, ids: &[MinigraphId], cx: &mut Context<Self>) {
        let mut any = false;
        for id in ids {
            any |= self.attach_quietly(*id);
        }
        if any {
            self.changed(cx);
        }
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

    /// True when a card with this source and column exists, attached or detached.
    pub fn has_source(&self, source: &MinigraphSource, column: GraphColumn) -> bool {
        self.entries
            .iter()
            .any(|e| e.column == column && &e.source == source)
    }

    /// Adds a process, name or app card at the next free slot and shows its toast: `Minigraph
    /// added`, `Process name minigraph added` or `App name minigraph added` (Success), `Minigraph
    /// already exists` (Info), `No space for more minigraphs` (Warning) or `Unable to add
    /// minigraph` (Error, for a `source` of `None`: the row could not be resolved). Persists.
    pub fn add_process(
        &mut self,
        source: Option<MinigraphSource>,
        column: GraphColumn,
        cx: &mut Context<Self>,
    ) -> Result<MinigraphId, AddError> {
        let Some(source) = source else {
            self.toast(ToastKind::Error, "minigraph-toast-unable", cx);
            return Err(AddError::Unavailable);
        };
        let added = match &source {
            MinigraphSource::Name(_) => "minigraph-toast-name-added",
            MinigraphSource::App(_) => "minigraph-toast-app-added",
            _ => "minigraph-toast-added",
        };
        match self.try_add(source, column) {
            Ok(id) => {
                self.changed(cx);
                self.toast(ToastKind::Success, added, cx);
                Ok(id)
            }
            Err(AddError::Exists) => {
                self.toast(ToastKind::Info, "minigraph-toast-exists", cx);
                Err(AddError::Exists)
            }
            Err(e) => {
                self.toast(ToastKind::Warning, "minigraph-toast-full", cx);
                Err(e)
            }
        }
    }

    /// Adds a card at the next free slot without a toast or a persist: `Exists` for a duplicate,
    /// `Full` when all slots are used.
    fn try_add(
        &mut self,
        source: MinigraphSource,
        column: GraphColumn,
    ) -> Result<MinigraphId, AddError> {
        if self.has_source(&source, column) {
            return Err(AddError::Exists);
        }
        self.insert(source, column)
            .map_err(|PanelFull| AddError::Full)
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
        let windows = self.windows.clone();
        TlSettings::update(cx, |d| {
            d.state.minigraphs = state;
            d.state.detached_windows = windows;
        });
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
        self.sync_windows(cx);
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
        self.toast_text(kind, text, cx);
    }

    fn toast_text(&mut self, kind: ToastKind, text: SharedString, cx: &mut Context<Self>) {
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

/// SplitMix64 step (Steele, Lea and Flood).
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Handles `AddRandomMinigraphs`: fills the free slots with CPU cards of random processes and
/// shows one result toast. Skips exited rows, kernel threads and this process. Returns the
/// number of cards added.
pub fn add_random(cx: &mut App) -> u32 {
    let Some(panel) = MinigraphPanel::global(cx) else {
        return 0;
    };
    panel.update(cx, |p, cx| {
        let self_pid = std::process::id();
        let mut keys: Vec<_> = p
            .hub
            .read(cx)
            .current()
            .processes
            .records
            .iter()
            .filter(|r| {
                !r.flags
                    .intersects(ProcFlags::EXITED | ProcFlags::KERNEL_THREAD | ProcFlags::IS_SELF)
                    && r.key.pid != self_pid
            })
            .map(|r| r.key)
            .collect();
        let mut seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        for i in (1..keys.len()).rev() {
            let j = (splitmix64(&mut seed) % (i as u64 + 1)) as usize;
            keys.swap(i, j);
        }
        let mut count = 0u32;
        for key in keys {
            match p.try_add(MinigraphSource::Process { key }, GraphColumn::Cpu) {
                Ok(_) => count += 1,
                Err(AddError::Full) => break,
                Err(_) => {}
            }
        }
        if count > 0 {
            p.changed(cx);
            let text = tr_args(
                cx,
                "prf-minigraph-random-added",
                &[("count", count.to_string().into())],
            );
            p.toast_text(ToastKind::Success, text, cx);
        } else {
            p.toast(ToastKind::Info, "prf-minigraph-random-none", cx);
        }
        count
    })
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
    app.on_action(move |a: &MinigraphDetach, cx| {
        let _ = panel.update(cx, |p, cx| p.detach(a.id, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphAttach, cx| {
        let _ = panel.update(cx, |p, cx| p.attach(a.id, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphAddToWindow, cx| {
        let _ = panel.update(cx, |p, cx| p.add_to_window(a.window, a.source.clone(), cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphMoveToSlot, cx| {
        let _ = panel.update(cx, |p, cx| p.move_to_slot(a.id, a.slot, cx));
    });
    let panel = weak.clone();
    app.on_action(move |a: &MinigraphSetColor, cx| {
        let _ = panel.update(cx, |p, cx| p.set_color(a.id, a.color, cx));
    });
    app.on_action(|_: &AddRandomMinigraphs, cx| {
        add_random(cx);
    });
    app.on_action(move |a: &MinigraphSetRange, cx| {
        let _ = weak.update(cx, |p, cx| p.set_range(a.id, a.secs, cx));
    });
}

/// True when `at` is more than `DETACH_EDGE` px outside a window of `size`.
fn outside_window(at: Point<Pixels>, size: Size<Pixels>) -> bool {
    let (x, y) = (f32::from(at.x), f32::from(at.y));
    x < -DETACH_EDGE
        || y < -DETACH_EDGE
        || x > f32::from(size.width) + DETACH_EDGE
        || y > f32::from(size.height) + DETACH_EDGE
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
            .on_action(cx.listener(|this, a: &MinigraphDetach, _, cx| this.detach(a.id, cx)))
            .on_action(cx.listener(|this, a: &MinigraphAttach, _, cx| this.attach(a.id, cx)))
            .on_action(cx.listener(|this, a: &MinigraphAddToWindow, _, cx| {
                this.add_to_window(a.window, a.source.clone(), cx)
            }))
            .on_drag_move(
                cx.listener(|this, ev: &DragMoveEvent<DraggedMinigraph>, _, cx| {
                    this.on_drag_move(ev, cx)
                }),
            )
            .on_drop(cx.listener(|this, _: &DraggedMinigraph, window, cx| {
                this.end_drag(window.mouse_position(), window.bounds().size, cx)
            }))
            // GPUI drops the drag on a release off the panel and fires no drop. A card's blocking
            // label row also hides the panel from the pointer, so the slot still decides.
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, window, cx| {
                    this.end_drag(ev.position, window.bounds().size, cx)
                }),
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
