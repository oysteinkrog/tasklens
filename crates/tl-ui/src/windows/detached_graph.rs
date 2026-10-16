//! The detached graph window (E26, PERF-083, UI-PRF-044, R-086): a borderless floating window
//! that holds one or more minigraph cards. Detaching, the menu, the KWin bridge and the
//! keep-open rule belong to later beads.

use std::sync::Arc;

use gpui_kit::base::Root;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Render, SharedString, Size, Styled as _, WeakEntity, Window, WindowBackgroundAppearance,
    WindowBounds, WindowDecorations, WindowHandle, WindowKind, WindowOptions, canvas, div, px,
    size,
};
use tl_model::settings::DetachedWindowState;

use crate::anim::AnimationDriver;
use crate::components::toast::ToastKind;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::pages::minigraph::card::{MINI_CARD_H, MINI_CARD_W, MinigraphCard};
use crate::pages::minigraph::panel::{MinigraphPanel, gpu_order};
use crate::pages::minigraph::sources::{MinigraphEntry, MinigraphId};
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
}

impl DetachedGraphWindow {
    fn new(
        window_id: u32,
        entries: Vec<MinigraphEntry>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        let hub = cx
            .try_global::<GlobalHub>()
            .expect("open_detached checked the hub")
            .0
            .clone();
        let catalog = Arc::clone(&hub.read(cx).current().devices);
        let order = gpu_order(cx);
        let cards = entries
            .into_iter()
            .map(|entry| {
                let id = entry.id;
                let hub = hub.clone();
                let card = cx.new(|cx| MinigraphCard::new(entry, &catalog, &order, &hub, cx));
                (id, card)
            })
            .collect();
        let panel = MinigraphPanel::global(cx)
            .map(|p| p.downgrade())
            .unwrap_or_else(WeakEntity::new_invalid);
        let title: SharedString = format!("tasklens Graph {window_id}").into();
        window.set_window_title(&title);
        Self {
            window_id,
            cards,
            driver,
            focus: cx.focus_handle(),
            panel,
            title,
        }
    }

    /// The content of a window opened by [`open_detached`], while it is open.
    pub fn find(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Self>> {
        let root = handle.downcast::<Root>()?;
        root.read(cx).ok()?.view().clone().downcast::<Self>().ok()
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
}

/// Opens detached window `window_id` with `cards`, or focuses it when it is open. `saved` gives
/// the size of a window restored from `[[detached_windows]]`. Failure shows a toast and returns
/// the error.
pub fn open_detached(
    window_id: u32,
    cards: Vec<MinigraphEntry>,
    saved: Option<&DetachedWindowState>,
    cx: &mut App,
) -> gpui_kit::Result<WindowHandle<Root>> {
    let toast_failed = |cx: &mut App| {
        let text = tr(cx, "minigraph-toast-detach-failed");
        push_main_toast(cx, ToastKind::Error, text, None, None);
    };
    // `WindowKey::DetachedGraph` holds a `u8`, so ids above 255 are refused.
    let (Ok(key_id), true) = (u8::try_from(window_id), cx.has_global::<GlobalHub>()) else {
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
            let this = cx.new(|cx| DetachedGraphWindow::new(window_id, cards, window, cx));
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        // Capture phase: a card's label row occludes and stops bubbling, and stopping here also
        // keeps the card's own drag from starting.
        let mover = canvas(
            |_, _, _| (),
            |_, _, window, _| {
                window.on_mouse_event(|ev: &MouseDownEvent, phase, window, cx| {
                    if phase.capture() && ev.button == MouseButton::Left {
                        start_move(window, cx);
                        cx.stop_propagation();
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
