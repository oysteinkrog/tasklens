//! The Performance mini window (E26, PERF-093, UI-PRF-045, R-086): a borderless floating window
//! that lists the visible sidebar cards in one column.
//!
//! The window owns a `DeviceSidebar` that is never rendered (leader decision on xdh.21). That
//! sidebar keeps the card list, the card text and the charts in step with the snapshot and the
//! settings, whether or not the Performance page was ever built. It is inactive, so it sends no
//! demand of its own; the mini window sends the sidebar's demand part with its own window
//! handle, so sampling goes on while the main window is hidden.
//!
//! Each card is a cached `CardView` over the private sidebar's chart. It differs from the sidebar
//! card: never selected, the pointer cursor on hover, no edit mode. Card clicks, the context
//! menu, "Always on top" through the KWin bridge and the View menu check belong to xdh.22.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::Root;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Pixels, Render,
    ScrollHandle, Size, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, div, px, size,
};
use smallvec::SmallVec;
use tl_model::demand::Demand;

use crate::anim::AnimationDriver;
use crate::chart::ChartView;
use crate::components::toast::ToastKind;
use crate::demand::{DemandAggregator, GlobalDemand};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr};
use crate::pages::minigraph::actions::CloseMiniWindow;
use crate::pages::performance::DeviceKey;
use crate::pages::performance::card::{CARD_H, CardLines, CardView};
use crate::pages::performance::sidebar::DeviceSidebar;
use crate::settings_global::TlSettings;
use crate::theme::{UiMetrics, current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// The window width at ui_scale 1 (PERF-093).
pub const MINI_W: f32 = 260.;
/// The card width at ui_scale 1: the window width less two `sp4`.
pub const MINI_CARD_W: f32 = MINI_W - 2. * SP4;
/// The `sp4` spacing at ui_scale 1: the window padding and the gap between cards.
const SP4: f32 = 8.;
/// The share of the work area height the window may take.
const HEIGHT_CAP: f32 = 0.9;

/// The window caption. Never translated: the KWin bridge matches it (R-086). The
/// `mini-window-title` string carries the same text.
pub const MINI_WINDOW_TITLE: &str = "tasklens Mini Window";
/// The key context of the window's one focus handle.
pub const MINI_CONTEXT: &str = "MiniWindow";
/// Element id of the scrolling card list.
pub const MINI_LIST_ID: &str = "mini-window-list";

/// The snapshot components the card text reads (the sidebar's text mask).
const TEXT_MASK: ComponentMask = ComponentMask(
    ComponentMask::SYSTEM.0
        | ComponentMask::INVENTORY.0
        | ComponentMask::DEVICE_DETAILS.0
        | ComponentMask::DEVICES.0,
);

/// The window height for cards of `card_heights` (logical px at ui_scale 1) at `ui_scale`
/// (UI-PRF-045): the sum of the card heights, each at least 80, plus `(n + 1) × sp4`, scaled,
/// capped at 90% of `work_area_h`. Beyond the cap the list scrolls.
pub fn mini_height(
    card_heights: impl IntoIterator<Item = f32>,
    work_area_h: f32,
    ui_scale: f32,
) -> f32 {
    let (n, sum) = card_heights
        .into_iter()
        .fold((0u32, 0.), |(n, sum), h| (n + 1, sum + h.max(CARD_H)));
    let content = (sum + (n as f32 + 1.) * SP4) * ui_scale;
    content.min(HEIGHT_CAP * work_area_h)
}

/// The options of a mini window of `size`. No position: KDE places it through the KWin bridge.
pub fn mini_window_options(size: Size<Pixels>, cx: &mut App) -> WindowOptions {
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

/// Binds Esc to `CloseMiniWindow` in `MiniWindow`. [`open_mini`] calls it; it binds once.
pub fn bind_keys_mini(cx: &mut App) {
    if cx.has_global::<MiniKeysBound>() {
        return;
    }
    cx.set_global(MiniKeysBound);
    cx.bind_keys([KeyBinding::new(
        "escape",
        CloseMiniWindow,
        Some(MINI_CONTEXT),
    )]);
}

struct MiniKeysBound;

impl gpui_kit::Global for MiniKeysBound {}

/// One card of the mini window.
pub struct MiniCard {
    pub key: DeviceKey,
    /// The private sidebar's chart for this card.
    pub chart: Entity<ChartView>,
    pub lines: CardLines,
    pub view: Entity<CardView>,
    /// The card height from its last render, scaled: at least `CARD_H`, more when its text
    /// wraps.
    pub height: Rc<Cell<Pixels>>,
}

pub struct MiniWindow {
    /// The visible sidebar cards, in sidebar order.
    cards: Vec<MiniCard>,
    driver: Entity<AnimationDriver>,
    /// On by default when the window opens; not persisted (PERF-093). xdh.22 applies it.
    always_on_top: bool,
    scroll: ScrollHandle,
    hub: Entity<SnapshotHub>,
    demand: Entity<DemandAggregator>,
    /// Never rendered and inactive. Owns the card list, text and charts.
    sidebar: Entity<DeviceSidebar>,
    focus: FocusHandle,
    window: AnyWindowHandle,
    /// The card width, scaled. Shared with every `CardView`.
    card_width: Rc<Cell<Pixels>>,
    /// The demand part last sent.
    part: Option<Demand>,
    /// The window size last asked for.
    size: Size<Pixels>,
    _subs: SmallVec<[Subscription; 5]>,
}

impl MiniWindow {
    fn new(
        sidebar: Entity<DeviceSidebar>,
        hub: Entity<SnapshotHub>,
        demand: Entity<DemandAggregator>,
        size: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let driver = cx.new(AnimationDriver::new);
        AnimationDriver::register(&driver, window, cx);
        window.set_window_title(MINI_WINDOW_TITLE);

        // The sidebar subscribed to the hub and the settings first, so its handlers have run
        // when these do.
        let mut subs = SmallVec::new();
        subs.push(cx.observe(&sidebar, |this, _, cx| this.sync(cx)));
        subs.push(cx.subscribe(&hub, |this, _, ev: &SnapshotChanged, cx| {
            if ev.mask.intersects(TEXT_MASK) {
                this.sync(cx);
            }
        }));
        subs.push(cx.observe_global::<TlSettings>(Self::sync));
        subs.push(cx.observe_global::<Strings>(Self::sync));
        subs.push(cx.observe_global::<UiMetrics>(Self::sync_style));
        let id = cx.entity_id();
        let release_demand = demand.clone();
        cx.on_release(move |_, cx| release_demand.update(cx, |d, cx| d.remove(id, cx)))
            .detach();

        let mut this = Self {
            cards: Vec::new(),
            driver,
            always_on_top: true,
            scroll: ScrollHandle::new(),
            hub,
            demand,
            sidebar,
            focus: cx.focus_handle(),
            window: window.window_handle(),
            card_width: Rc::new(Cell::new(px(MINI_CARD_W * metrics(cx).ui_scale))),
            part: None,
            size,
            _subs: subs,
        };
        this.sync(cx);
        this
    }

    /// The content of a window opened by [`open_mini`], while it is open.
    pub fn find(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Self>> {
        let root = handle.downcast::<Root>()?;
        root.read(cx).ok()?.view().clone().downcast::<Self>().ok()
    }

    /// The open mini window's content, if any.
    pub fn open_view(cx: &App) -> Option<Entity<Self>> {
        let registry = cx.try_global::<GlobalRegistry>()?.0.read(cx);
        Self::find(registry.get(&WindowKey::Mini)?, cx)
    }

    /// This window's own animation driver (one per window, ADR-009).
    pub fn driver(&self) -> &Entity<AnimationDriver> {
        &self.driver
    }

    pub fn cards(&self) -> &[MiniCard] {
        &self.cards
    }

    /// The card keys in list order.
    pub fn keys(&self) -> impl Iterator<Item = &DeviceKey> {
        self.cards.iter().map(|c| &c.key)
    }

    pub fn always_on_top(&self) -> bool {
        self.always_on_top
    }

    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub fn sidebar(&self) -> &Entity<DeviceSidebar> {
        &self.sidebar
    }

    pub fn hub(&self) -> &Entity<SnapshotHub> {
        &self.hub
    }

    /// The demand part this window sent.
    pub fn demand_part(&self) -> Option<&Demand> {
        self.part.as_ref()
    }

    /// Follows the private sidebar: makes a card for a new key, drops a gone one, keeps the
    /// order, pushes changed text, and sends the demand part when it changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let sidebar = self.sidebar.read(cx);
        let wanted: Vec<(DeviceKey, Entity<ChartView>, CardLines)> = sidebar
            .cards()
            .iter()
            .filter(|c| !c.is_removing())
            .map(|c| (c.row.key.clone(), c.chart.clone(), c.lines.clone()))
            .collect();
        let part = sidebar.demand_part();

        let id = cx.entity_id();
        let mut old = std::mem::take(&mut self.cards);
        let mut listed = old.len() != wanted.len();
        for (key, chart, lines) in wanted {
            let found = old.iter().position(|c| c.key == key && c.chart == chart);
            let card = match found {
                Some(p) => {
                    listed |= p != 0;
                    let mut card = old.remove(p);
                    if card.lines != lines {
                        card.lines = lines.clone();
                        card.view.update(cx, |v, cx| v.set_lines(lines, cx));
                    }
                    card
                }
                None => {
                    listed = true;
                    let height = Rc::new(Cell::new(px(CARD_H * metrics(cx).ui_scale)));
                    let view = cx.new(|cx| {
                        let mut view = CardView::new(
                            key.clone(),
                            chart.clone(),
                            Rc::clone(&self.card_width),
                            Rc::clone(&height),
                            id,
                        );
                        view.set_pointer(true, cx);
                        view.set_lines(lines.clone(), cx);
                        view
                    });
                    MiniCard {
                        key,
                        chart,
                        lines,
                        view,
                        height,
                    }
                }
            };
            self.cards.push(card);
        }
        if listed {
            cx.notify();
        }
        if self.part.as_ref() != Some(&part) {
            let window = Some(self.window);
            self.part = Some(part.clone());
            self.demand.update(cx, |d, cx| d.set(id, window, part, cx));
        }
    }

    /// A new theme or UI scale: the cards take the new colours and width.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        self.card_width.set(px(MINI_CARD_W * metrics(cx).ui_scale));
        for card in &self.cards {
            card.view.update(cx, CardView::invalidate_style);
        }
        cx.notify();
    }

    fn on_close(&mut self, _: &CloseMiniWindow, window: &mut Window, _: &mut Context<Self>) {
        // The registry drops `Mini` when the window closes.
        window.remove_window();
    }
}

/// The work area height of `window`'s output, or of the primary output.
fn work_area_h(window: Option<&Window>, cx: &App) -> f32 {
    window
        .and_then(|w| w.display(cx))
        .or_else(|| cx.primary_display())
        .map_or(f32::INFINITY, |d| f32::from(d.visible_bounds().size.height))
}

/// Opens the mini window, or focuses it when it is open (UI-FND-021). Failure shows the toast
/// "Could not open the mini window".
pub fn open_mini(cx: &mut App) -> OpenOutcome {
    let toast_failed = |cx: &mut App| {
        let text = tr(cx, "mini-window-toast-open-failed");
        push_main_toast(cx, ToastKind::Error, text, None, None);
    };
    let globals = cx
        .try_global::<GlobalHub>()
        .map(|h| h.0.clone())
        .zip(cx.try_global::<GlobalDemand>().map(|d| d.0.clone()));
    let Some((hub, demand)) = globals else {
        toast_failed(cx);
        return OpenOutcome::Failed;
    };
    bind_keys_mini(cx);
    let open = |cx: &mut App| {
        let sidebar = cx.new(|cx| {
            let mut sidebar = DeviceSidebar::new(hub.clone(), demand.clone(), cx);
            sidebar.set_active(false, cx);
            sidebar.set_saves_selection(false);
            sidebar
        });
        let s = metrics(cx).ui_scale;
        let heights: Vec<f32> = sidebar
            .read(cx)
            .cards()
            .iter()
            .filter(|c| !c.is_removing())
            .map(|c| f32::from(c.height.get()) / s)
            .collect();
        let size = size(
            px(MINI_W * s),
            px(mini_height(heights, work_area_h(None, cx), s)),
        );
        let options = mini_window_options(size, cx);
        let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
            let this = cx.new(|cx| MiniWindow::new(sidebar, hub, demand, size, window, cx));
            let focus = this.read(cx).focus.clone();
            window.focus(&focus, cx);
            this
        })?;
        Ok(handle)
    };
    let outcome = match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Mini, cx, open)),
        None => match open(cx) {
            Ok(_) => OpenOutcome::Opened,
            Err(err) => {
                tracing::warn!(target: "tasklens::ui", %err, "could not open the mini window");
                OpenOutcome::Failed
            }
        },
    };
    if matches!(outcome, OpenOutcome::Failed | OpenOutcome::CapReached(_)) {
        toast_failed(cx);
    }
    outcome
}

impl Render for MiniWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let width = self.card_width.get();

        // The cards measured their heights on their last render; each notifies this view when
        // its height changed. The window follows the sum, up to the cap.
        let heights = self.cards.iter().map(|c| f32::from(c.height.get()) / s);
        let want = size(
            px(MINI_W * s),
            px(mini_height(heights, work_area_h(Some(window), cx), s)),
        );
        if want != self.size {
            self.size = want;
            cx.defer_in(window, move |_, window, _| window.resize(want));
        }

        let list = v_flex()
            .id(MINI_LIST_ID)
            .size_full()
            .p(m.sp[4])
            .gap(m.sp[4])
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(self.cards.iter().map(|c| {
                div().flex_none().child(
                    c.view
                        .clone()
                        .cached(StyleRefinement::default().w(width).h(c.height.get())),
                )
            }));
        // The border is an overlay: `border_1` on the root would take 2 px from the layout.
        let border = div()
            .absolute()
            .inset_0()
            .border_1()
            .border_color(t.divider);
        div()
            .id("mini-window")
            .debug_selector(|| "mini-window".into())
            .key_context(MINI_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_close))
            .relative()
            .size_full()
            .bg(t.window_bg)
            .child(list)
            .child(border)
            .vertical_scrollbar(&self.scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_window_height_cap() {
        assert_eq!(mini_height([80.; 9], 1000., 1.), 800.);
        assert_eq!(mini_height([80.; 15], 1000., 1.), 900.);
        // A card whose text wraps counts at its own height; a shorter one counts as 80.
        assert_eq!(mini_height([80., 96.], 1000., 1.), 200.);
        assert_eq!(mini_height([60.], 1000., 1.), 96.);
        // Scaled, then capped at 90% of the work area.
        assert_eq!(mini_height([80.; 2], 1000., 1.5), 276.);
        assert_eq!(mini_height([80.; 9], 1000., 1.5), 900.);
        assert_eq!(mini_height([], 1000., 1.), 8.);
    }
}
