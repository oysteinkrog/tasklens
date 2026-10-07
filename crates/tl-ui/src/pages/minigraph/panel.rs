//! `MinigraphPanel`: the minigraph cards over the Processes table (E26-004, PERF-080, PERF-082,
//! APP-021, R-089, UI-PRF-040, ADR-011).
//!
//! The panel owns the cards and places them in the 4 × 3 slot grid from the bottom right. It
//! follows the `graphs.minigraph_*` settings, writes `[[minigraphs]]` on every change of the
//! card set (tl-app debounces the write by 500 ms), handles `ResetMinigraphLayout`,
//! `MinigraphHide` and `MinigraphRemove`, and sends the cards' demand as one surface while the
//! Processes tab is shown (as the process table does, pdu.52). It re-renders only when the card
//! set changes; each card is a cached view that re-renders on its own.

use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StyleRefinement, Styled as _, Subscription, WeakEntity, Window,
    div, px,
};
use tl_model::demand::{CpuDemand, Demand};
use tl_model::settings::GraphsSettings;

use super::GraphColor;
use super::actions::{MinigraphHide, MinigraphRemove, MinigraphSetRange, ResetMinigraphLayout};
use super::card::{MINI_CARD_H, MINI_CARD_W, MinigraphCard};
use super::sources::{
    GraphColumn, MAX_SLOTS, MinigraphEntry, MinigraphId, MinigraphSource, default_entries,
    from_state, slot_cell, to_state,
};
use crate::components::toast::ToastKind;
use crate::demand::DemandAggregator;
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::pages::performance::{DeviceKey, DeviceKind};
use crate::settings_global::{SettingValue, TlSettings};
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
        let source = cx.entity_id();
        cx.on_release(move |this, cx| {
            this.demand.update(cx, |a, cx| a.remove(source, cx));
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
    /// withdraws it.
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
    pub fn demand_part(&self, cx: &App) -> Demand {
        let mut d = Demand::default();
        for (_, card) in &self.cards {
            let p = card.read(cx).demand_part();
            d.any_surface_visible |= p.any_surface_visible;
            if p.cpu != CpuDemand::Off {
                d.cpu = CpuDemand::Total;
            }
            d.memory |= p.memory;
            d.disks |= p.disks;
            d.nets |= p.nets;
            d.gpus |= p.gpus;
        }
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

    /// The card set changed: rebuild the cards, resend demand, persist, redraw.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.rebuild_cards(cx);
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
                    cx.new(|cx| MinigraphCard::new(entry, &catalog, &order, &hub, cx))
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
    app.on_action(move |a: &MinigraphSetRange, cx| {
        let _ = weak.update(cx, |p, cx| p.set_range(a.id, a.secs, cx));
    });
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
fn gpu_order(cx: &App) -> Vec<DeviceKey> {
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
        // No handlers on the panel itself, so it is no hitbox: only the cards take the pointer.
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
            );
        if self.cards.is_empty() {
            return root;
        }
        let s = metrics(cx).ui_scale;
        let (w, h) = (MINI_CARD_W * s, MINI_CARD_H * s);
        for (id, card) in &self.cards {
            let slot = card.read(cx).entry.slot;
            let (row, col) = slot_cell(slot);
            let right = (MARGIN + f32::from(col) * (MINI_CARD_W + GAP)) * s;
            let bottom = (MARGIN + f32::from(row) * (MINI_CARD_H + GAP)) * s;
            root = root.child(
                div()
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
                    ),
            );
        }
        root
    }
}
