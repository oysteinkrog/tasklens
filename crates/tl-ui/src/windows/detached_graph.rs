//! The detached graph window (E26, PERF-083, UI-PRF-044, R-086): a borderless floating window
//! that holds one or more minigraph cards. The panel opens, fills and closes these windows
//! (E26-015). A right-click opens the window menu (E26-018). Each window sends its own demand
//! part, so the app keeps sampling for it while the main window is hidden (PERF-084, E26-020).

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, Size, Styled as _, WeakEntity, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowHandle, WindowKind, WindowOptions, canvas, div, px,
    size,
};
use tl_model::demand::Demand;
use tl_model::settings::DetachedWindowState;

use crate::actions::{Quit, ShowMainWindow};
use crate::anim::AnimationDriver;
use crate::components::menu::{
    TlMenu, context_menu_layer, context_menu_open, open_context_menu_at,
};
use crate::components::toast::ToastKind;
use crate::host::HostContext;
use crate::hub::SnapshotHub;
use crate::i18n::tr;
use crate::pages::minigraph::actions::{
    MinigraphAddToWindow, MinigraphAttach, MinigraphCloseDetachedWindow, MinigraphHide,
    MinigraphRemove,
};
use crate::pages::minigraph::bridge::OwnWindowBridge;
use crate::pages::minigraph::card::{CardEnded, MINI_CARD_H, MINI_CARD_W, MinigraphCard};
use crate::pages::minigraph::menu::{color_submenu, range_submenu};
use crate::pages::minigraph::panel::{MinigraphPanel, SYSTEM, cards_demand, gpu_order, same_kind};
use crate::pages::minigraph::sources::{MinigraphEntry, MinigraphId, MinigraphSource};
use crate::theme::{current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// The `sp[2]` spacing at ui_scale 1: the window padding and the gap between cards.
const SP2: f32 = 4.;

/// The window size for `n_cards` stacked cards at `ui_scale` (UI-PRF-044):
/// `(180 + 2 sp2) × (n × 90 + (n + 1) sp2)`, scaled.
pub fn window_size(n_cards: usize, ui_scale: f32) -> Size<Pixels> {
    let n = n_cards as f32;
    size(
        px((MINI_CARD_W + 2. * SP2) * ui_scale),
        px((n * MINI_CARD_H + (n + 1.) * SP2) * ui_scale),
    )
}

/// The card at `pos` (window coordinates) in a window of `n_cards` stacked cards at `ui_scale`,
/// or `None` on the padding or in a gap. Card `i` covers x `4..184` and y `4 + 94 i .. 94 + 94 i`
/// at scale 1.
pub fn card_at(pos: Point<Pixels>, n_cards: usize, ui_scale: f32) -> Option<usize> {
    let x = f32::from(pos.x) / ui_scale;
    let y = f32::from(pos.y) / ui_scale;
    if !(SP2..SP2 + MINI_CARD_W).contains(&x) {
        return None;
    }
    let step = MINI_CARD_H + SP2;
    let i = ((y - SP2) / step).floor();
    if i < 0. || (y - SP2) - i * step >= MINI_CARD_H {
        return None;
    }
    let i = i as usize;
    (i < n_cards).then_some(i)
}

/// The options of a detached graph window of `size`. No position: KDE places it through the
/// KWin bridge.
pub fn detached_window_options(size: Size<Pixels>, cx: &mut App) -> WindowOptions {
    WindowOptions {
        kind: WindowKind::Floating,
        window_decorations: Some(WindowDecorations::Client),
        titlebar: None,
        is_resizable: false,
        is_movable: true,
        app_id: Some(crate::app::window_app_id(cx)),
        window_background: WindowBackgroundAppearance::Opaque,
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        ..Default::default()
    }
}

pub struct DetachedGraphWindow {
    pub window_id: u32,
    cards: Vec<(MinigraphId, Entity<MinigraphCard>)>,
    driver: Entity<AnimationDriver>,
    focus: FocusHandle,
    panel: WeakEntity<MinigraphPanel>,
    /// The window caption. Never translated: the KWin bridge matches it (R-086).
    title: SharedString,
    /// The host this window shows: its hub feeds the cards and its demand aggregator gets the
    /// window's part. Set when it opens; never changes.
    host: HostContext,
    window: AnyWindowHandle,
    /// The part last sent, to send only a change.
    part: Option<Demand>,
}

impl DetachedGraphWindow {
    fn new(
        host: &HostContext,
        window_id: u32,
        entries: Vec<MinigraphEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        let hub = host.hub().expect("open_detached checked the hub").clone();
        let cards: Vec<_> = entries
            .into_iter()
            .map(|entry| {
                let id = entry.id;
                let card = make_card(entry, &hub, cx);
                Self::watch_card(&card, cx);
                (id, card)
            })
            .collect();
        let panel = MinigraphPanel::global(cx)
            .map(|p| p.downgrade())
            .unwrap_or_else(WeakEntity::new_invalid);
        let title: SharedString = host.titled(&format!("tasklens Graph {window_id}")).into();
        window.set_window_title(&title);
        if let Some(demand) = host.demand().cloned() {
            let id = cx.entity_id();
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }
        let mut this = Self {
            window_id,
            cards,
            driver,
            focus: cx.focus_handle(),
            panel,
            title,
            host: host.clone(),
            window: window.window_handle(),
            part: None,
        };
        this.send_demand(cx);
        this
    }

    /// An ended card drops its graph target, so its owner resends demand (PERF-086).
    fn watch_card(card: &Entity<MinigraphCard>, cx: &mut Context<Self>) {
        cx.subscribe(card, |this, _, _: &CardEnded, cx| this.send_demand(cx))
            .detach();
    }

    /// Sends this window's part: the sum of its cards. Sends only a change.
    fn send_demand(&mut self, cx: &mut Context<Self>) {
        let Some(demand) = self.host.demand().cloned() else {
            return;
        };
        let part = cards_demand(self.cards.iter().map(|(_, c)| c), cx);
        if self.part.as_ref() == Some(&part) {
            return;
        }
        self.part = Some(part.clone());
        let (id, window) = (cx.entity_id(), Some(self.window));
        demand.update(cx, |d, cx| d.set(id, window, part, cx));
    }

    /// The content of a window opened by [`open_detached`], while it is open.
    pub fn find(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Self>> {
        let root = handle.downcast::<Root>()?;
        root.read(cx).ok()?.view().clone().downcast::<Self>().ok()
    }

    /// Shows `entries`, in this order: a card that is already here stays, a missing one is
    /// created, the others are dropped.
    pub fn set_cards(&mut self, entries: Vec<MinigraphEntry>, cx: &mut Context<Self>) {
        let hub = self
            .host
            .hub()
            .expect("open_detached checked the hub")
            .clone();
        let mut old = std::mem::take(&mut self.cards);
        for entry in entries {
            let id = entry.id;
            let card = match old.iter().position(|(c, _)| *c == id) {
                Some(i) => old.swap_remove(i).1,
                None => {
                    let card = make_card(entry, &hub, cx);
                    Self::watch_card(&card, cx);
                    card
                }
            };
            self.cards.push((id, card));
        }
        self.send_demand(cx);
        cx.notify();
    }

    /// This window's own animation driver (one per window, ADR-009).
    pub fn driver(&self) -> &Entity<AnimationDriver> {
        &self.driver
    }

    pub fn cards(&self) -> &[(MinigraphId, Entity<MinigraphCard>)] {
        &self.cards
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn panel(&self) -> &WeakEntity<MinigraphPanel> {
        &self.panel
    }

    /// Opens the window menu at `at`: for card `card` (an index into the cards), or for the
    /// body between cards when `None` (PERF-083, UI-PRF-044).
    pub fn open_menu(
        &self,
        at: Point<Pixels>,
        card: Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let target = card
            .and_then(|i| self.cards.get(i))
            .map(|(_, c)| c.read(cx).entry.clone());
        let shown: Vec<MinigraphSource> = self
            .cards
            .iter()
            .map(|(_, c)| c.read(cx).entry.source.clone())
            .collect();
        let window_id = self.window_id;
        let context = window.focused(cx).unwrap_or_else(|| self.focus.clone());
        let menu = TlMenu::build(window, cx, context, move |menu, window, cx| {
            detached_menu(menu, window_id, target.as_ref(), &shown, window, cx)
        });
        open_context_menu_at(at, window, cx, menu);
    }
}

/// The label of a system source in the Add graph submenu.
fn source_label(source: &MinigraphSource, cx: &App) -> SharedString {
    let key = match source {
        MinigraphSource::Cpu => "minigraph-label-cpu",
        MinigraphSource::Memory => "minigraph-label-memory",
        MinigraphSource::Disk => "minigraph-label-disk",
        MinigraphSource::Network => "minigraph-label-network",
        MinigraphSource::Gpu(_) => "minigraph-label-gpu",
        MinigraphSource::Vram(_) => "minigraph-label-vram",
        MinigraphSource::GpuTemp(_) => "minigraph-label-gpu-temp",
        _ => return SharedString::default(),
    };
    tr(cx, key)
}

/// The detached window menu in UI-PRF-044 order. `target` is the clicked card; the per-card
/// items are left out without one. `shown` lists the sources in this window.
fn detached_menu(
    menu: TlMenu,
    window_id: u32,
    target: Option<&MinigraphEntry>,
    shown: &[MinigraphSource],
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let mut menu = menu
        .item(tr(cx, "minigraph-menu-show-main"), Box::new(ShowMainWindow))
        .item(
            tr(cx, "minigraph-menu-close-graph"),
            Box::new(MinigraphCloseDetachedWindow { window: window_id }),
        );
    if let Some(entry) = target {
        let id = entry.id;
        menu = menu.item(
            tr(cx, "minigraph-menu-attach"),
            Box::new(MinigraphAttach { id }),
        );
        menu = range_submenu(menu, id, entry.range_s, true, window, cx);
        menu = color_submenu(menu, id, entry.color, window, cx);
    }
    let missing: Vec<(MinigraphSource, SharedString)> = SYSTEM
        .iter()
        .filter(|(s, _)| !shown.iter().any(|o| same_kind(o, s)))
        .map(|(s, _)| (s.clone(), source_label(s, cx)))
        .collect();
    let label = tr(cx, "minigraph-menu-add-graph");
    menu = if missing.is_empty() {
        menu.item_disabled(label, None)
    } else {
        menu.submenu(label, window, cx, move |mut menu, _, _| {
            for (source, name) in &missing {
                menu = menu.item(
                    name.clone(),
                    Box::new(MinigraphAddToWindow {
                        window: window_id,
                        source: source.clone(),
                    }),
                );
            }
            menu
        })
    };
    if let Some(entry) = target {
        let id = entry.id;
        // A system card is hidden, so its View > Graphs setting turns off.
        menu = if entry.source.is_system() {
            menu.item(
                tr(cx, "minigraph-menu-remove"),
                Box::new(MinigraphHide { id }),
            )
        } else {
            menu.item(
                tr(cx, "minigraph-menu-remove"),
                Box::new(MinigraphRemove { id }),
            )
        };
    }
    menu.item(tr(cx, "minigraph-menu-exit"), Box::new(Quit))
}

fn make_card(
    entry: MinigraphEntry,
    hub: &Entity<SnapshotHub>,
    cx: &mut App,
) -> Entity<MinigraphCard> {
    let catalog = Arc::clone(&hub.read(cx).current().devices);
    let order = gpu_order(cx);
    cx.new(|cx| MinigraphCard::new(entry, &catalog, &order, hub, cx))
}

/// Opens detached window `window_id` with `cards`, or focuses it when it is open. `saved` gives
/// the size of a window restored from `[[detached_windows]]`. Failure shows a toast and returns
/// the error. The window shows `host`, the host of the page the cards came from, and stays on it.
pub fn open_detached(
    host: &HostContext,
    window_id: u32,
    cards: Vec<MinigraphEntry>,
    saved: Option<&DetachedWindowState>,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    OwnWindowBridge::ensure(cx);
    let toast_failed = |cx: &mut App| {
        let text = tr(cx, "minigraph-toast-detach-failed");
        push_main_toast(cx, ToastKind::Error, text, None, None);
    };
    // `WindowKey::DetachedGraph` holds a `u8`, so ids above 255 are refused.
    let (Ok(key_id), true) = (u8::try_from(window_id), host.hub().is_some()) else {
        toast_failed(cx);
        return Err(gpui_kit::gpui::private::anyhow::anyhow!(
            "detached graph window id {window_id} refused"
        ));
    };
    let key = WindowKey::DetachedGraph(key_id);
    let window_size = match saved {
        Some(s) => size(px(s.width as f32), px(s.height as f32)),
        None => window_size(cards.len(), metrics(cx).ui_scale),
    };
    let mut opened: Option<AnyWindowHandle> = None;
    let open = |cx: &mut App| {
        let options = detached_window_options(window_size, cx);
        let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
            let this = cx.new(|cx| DetachedGraphWindow::new(host, window_id, cards, window, cx));
            let focus = this.read(cx).focus.clone();
            window.focus(&focus, cx);
            this
        })?;
        opened = Some(handle);
        Ok(handle)
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            let outcome = registry.update(cx, |r, cx| r.open_or_focus(key.clone(), cx, open));
            match outcome {
                OpenOutcome::Opened => {}
                OpenOutcome::Focused => opened = registry.read(cx).get(&key),
                _ => {
                    toast_failed(cx);
                    return Err(gpui_kit::gpui::private::anyhow::anyhow!(
                        "could not open detached graph {window_id}"
                    ));
                }
            }
        }
        None => {
            if let Err(err) = open(cx) {
                toast_failed(cx);
                return Err(err);
            }
        }
    }
    let handle = opened.expect("a window was opened or found");
    Ok(WindowHandle::<Root>::new(handle.window_id()))
}

fn start_move(window: &Window, cx: &mut App) {
    #[cfg(any(test, feature = "test-hooks"))]
    if cx.has_global::<crate::window::titlebar::TitleBarProbe>() {
        cx.global_mut::<crate::window::titlebar::TitleBarProbe>()
            .moves += 1;
        return;
    }
    let _ = cx;
    window.start_window_move();
}

impl Render for DetachedGraphWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let this = cx.weak_entity();
        let n_cards = self.cards.len();
        let ui_scale = m.ui_scale;
        // Capture phase: a card's label row occludes and stops bubbling, and stopping here also
        // keeps the card's own drag from starting.
        let mover = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                    // An open menu takes its own clicks.
                    if !phase.capture() || context_menu_open(window, cx) {
                        return;
                    }
                    match ev.button {
                        MouseButton::Left => {
                            start_move(window, cx);
                            cx.stop_propagation();
                        }
                        MouseButton::Right => {
                            let card = card_at(ev.position, n_cards, ui_scale);
                            let at = ev.position;
                            let _ = this.update(cx, |v, cx| v.open_menu(at, card, window, cx));
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                });
            },
        )
        .absolute()
        .size_full();
        // The border is an overlay: `border_1` on the root would take 2 px from the layout.
        let border = div()
            .absolute()
            .inset_0()
            .border_1()
            .border_color(t.divider);
        div()
            .size_full()
            .relative()
            .bg(t.window_bg)
            .p(m.sp[2])
            .flex()
            .flex_col()
            .gap(m.sp[2])
            .track_focus(&self.focus)
            .child(mover)
            .children(self.cards.iter().map(|(_, card)| card.clone()))
            .child(border)
            .children(context_menu_layer(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_window_size_for_two_cards() {
        assert_eq!(window_size(2, 1.), size(px(188.), px(192.)));
        assert_eq!(window_size(1, 1.5), size(px(282.), px(147.)));
    }
}
