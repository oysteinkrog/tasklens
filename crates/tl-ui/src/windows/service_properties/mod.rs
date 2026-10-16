//! Service Properties window (UI-SYS-030 to UI-SYS-038).
//!
//! This file is the frame: the five tabs, the OK, Cancel and Apply footer, the window cap, the
//! demand for the unit's details and the apply flow. The tab contents live in their own beads;
//! a tab that has no view yet shows an empty area. The window pushes each new details Arc into
//! every built tab view with `set_details`.

mod dependencies;
mod general;
mod logon;
mod recovery;

pub use dependencies::DependenciesTab;
pub use general::GeneralTab;
pub use logon::LogOnTab;
pub use recovery::RecoveryTab;

use std::rc::Rc;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, EntityId, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StyleRefinement, Styled as _,
    Subscription, Window, div, px, size,
};
use smallvec::SmallVec;
use tl_model::actions::ActionId;
use tl_model::demand::Demand;
use tl_model::snapshot::{
    ComponentMask, ServiceBackend, ServicesSnapshot, UnitDetails, UnitRecord,
};
use tl_model::table::UnitRef;
use tracing::warn;

use crate::action_tracker::{ActionTrackerEvent, GlobalActionTracker};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::tabs::{TlTabSpec, TlTabs};
use crate::components::toast::ToastKind;
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::demand::GlobalDemand;
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::{tr, tr_args};
use crate::pages::services::actions::{
    SERVICE_PROPS_CONTEXT, ServicePropsNextTab, ServicePropsOk, ServicePropsPrevTab,
    ServicePropsSelectTab,
};
use crate::pages::sys_table::SysActions;
use crate::theme::{current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// Default window size at 100% scale.
const WINDOW_SIZE: (f32, f32) = (720., 560.);
/// Smallest window size at 100% scale (UI-SYS-030).
const MIN_SIZE: (f32, f32) = (560., 440.);
/// Footer height at 100% scale.
const FOOTER_H: f32 = 48.;

/// The tabs of the Service Properties window, in strip order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServicePropsTab {
    General,
    LogOn,
    Recovery,
    Dependencies,
    Logs,
}

const ALL_TABS: [ServicePropsTab; 5] = [
    ServicePropsTab::General,
    ServicePropsTab::LogOn,
    ServicePropsTab::Recovery,
    ServicePropsTab::Dependencies,
    ServicePropsTab::Logs,
];

impl ServicePropsTab {
    /// Position in the strip, also the `ServicePropsSelectTab` index.
    pub fn index(self) -> usize {
        match self {
            Self::General => 0,
            Self::LogOn => 1,
            Self::Recovery => 2,
            Self::Dependencies => 3,
            Self::Logs => 4,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Self::General => "services-sys030-tab-general",
            Self::LogOn => "services-sys030-tab-logon",
            Self::Recovery => "services-sys030-tab-recovery",
            Self::Dependencies => "services-sys030-tab-dependencies",
            Self::Logs => "services-sys030-tab-logs",
        }
    }
}

/// What each tab view implements so the footer can apply it.
pub trait ServicePropsTabView {
    fn is_dirty(&self, cx: &App) -> bool;

    fn is_valid(&self, _cx: &App) -> bool {
        true
    }

    /// Starts the apply and returns the action ids it submitted. A tab that returns none and is
    /// still dirty was not applied (the unit was busy, say): the window treats it as a failed
    /// tab, keeps its edit and does not close.
    fn apply(&mut self, window: &mut Window, cx: &mut App) -> SmallVec<[ActionId; 2]>;

    fn reset(&mut self, cx: &mut App);

    /// The unit's latest details (`None` until they arrive) and whether the unit is gone or not
    /// loaded. Called once after the view is built and again on every change. A view drawn
    /// through `.cached()` notifies itself here.
    fn set_details(
        &mut self,
        _details: Option<&Arc<UnitDetails>>,
        _not_loaded: bool,
        _cx: &mut App,
    ) {
    }
}

/// The footer's view of a tab entity, without the tab's concrete type.
trait TabHandle {
    fn is_dirty(&self, cx: &App) -> bool;
    fn is_valid(&self, cx: &App) -> bool;
    fn apply(&self, window: &mut Window, cx: &mut App) -> SmallVec<[ActionId; 2]>;
    fn reset(&self, cx: &mut App);
    fn set_details(&self, details: Option<&Arc<UnitDetails>>, not_loaded: bool, cx: &mut App);
}

impl<T: Render + ServicePropsTabView> TabHandle for Entity<T> {
    fn is_dirty(&self, cx: &App) -> bool {
        self.read(cx).is_dirty(cx)
    }

    fn is_valid(&self, cx: &App) -> bool {
        self.read(cx).is_valid(cx)
    }

    fn apply(&self, window: &mut Window, cx: &mut App) -> SmallVec<[ActionId; 2]> {
        self.update(cx, |tab, cx| tab.apply(window, cx))
    }

    fn reset(&self, cx: &mut App) {
        self.update(cx, |tab, cx| tab.reset(cx));
    }

    fn set_details(&self, details: Option<&Arc<UnitDetails>>, not_loaded: bool, cx: &mut App) {
        self.update(cx, |tab, cx| tab.set_details(details, not_loaded, cx));
    }
}

struct TabSlot {
    view: AnyView,
    handle: Rc<dyn TabHandle>,
}

impl TabSlot {
    fn new<T: Render + ServicePropsTabView>(view: Entity<T>) -> Self {
        Self {
            view: AnyView::from(view.clone()),
            handle: Rc::new(view),
        }
    }
}

/// One slot per tab, filled the first time the tab is shown.
type TabViews = [Option<TabSlot>; 5];

/// What a tab view is built with.
struct TabInput {
    unit: UnitRef,
    /// The window's action layer.
    actions: Entity<SysActions>,
    /// The window view, notified when a tab's dirty state changes.
    parent: EntityId,
}

/// Builds the view of `tab`. Each tab bead adds its arm; until then a tab has no view.
fn create_tab(
    tab: ServicePropsTab,
    input: TabInput,
    window: &mut Window,
    cx: &mut App,
) -> Option<TabSlot> {
    match tab {
        ServicePropsTab::General => {
            let view =
                cx.new(|cx| GeneralTab::new(input.unit, input.actions, input.parent, window, cx));
            Some(TabSlot::new(view))
        }
        ServicePropsTab::LogOn => {
            let view = cx.new(LogOnTab::new);
            Some(TabSlot::new(view))
        }
        ServicePropsTab::Recovery => {
            let view = cx.new(RecoveryTab::new);
            Some(TabSlot::new(view))
        }
        ServicePropsTab::Dependencies => {
            let view = cx.new(DependenciesTab::new);
            Some(TabSlot::new(view))
        }
        ServicePropsTab::Logs => None,
    }
}

/// True when `unit` is not in the snapshot or systemd did not load it.
fn unit_not_loaded(units: &[UnitRecord], unit: &UnitRef) -> bool {
    !units
        .iter()
        .any(|record| record.unit == *unit && &*record.load_state == "loaded")
}

fn title_for(unit: &UnitRef, not_loaded: bool, cx: &App) -> SharedString {
    let key = if not_loaded {
        "services-sys030-title-not-loaded"
    } else {
        "services-sys030-title"
    };
    tr_args(cx, key, &[("id", FluentValue::from(&*unit.name))])
}

pub struct ServicePropertiesWindow {
    unit: UnitRef,
    /// Pointer-compared on every SERVICES change.
    details: Option<Arc<UnitDetails>>,
    /// The `units` list of the last SERVICES snapshot, pointer-compared.
    units: Arc<[UnitRecord]>,
    backend: ServiceBackend,
    not_loaded: bool,
    active: ServicePropsTab,
    tabs: TabViews,
    /// This window's own action layer: its toasts show in this window.
    actions: Entity<SysActions>,
    applying: SmallVec<[(ActionId, ServicePropsTab); 4]>,
    /// The first tab, in strip order, that has a failed action in the running apply.
    failed_tab: Option<ServicePropsTab>,
    close_after_apply: bool,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u32,
    _subs: Vec<Subscription>,
}

impl ServicePropertiesWindow {
    /// Opens or focuses the window for `unit`, on `tab`.
    pub fn open(unit: UnitRef, tab: ServicePropsTab, cx: &mut App) {
        let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
            warn!(target: "tasklens::ui", "no window registry, service properties not opened");
            return;
        };
        let not_loaded = cx
            .try_global::<GlobalHub>()
            .is_some_and(|hub| unit_not_loaded(&hub.0.read(cx).current().services.units, &unit));
        let s = metrics(cx).ui_scale;
        let key = WindowKey::Service(unit.clone());
        let spec = TlWindowSpec::new(
            TlWindowKind::Properties,
            title_for(&unit, not_loaded, cx),
            size(px(WINDOW_SIZE.0 * s), px(WINDOW_SIZE.1 * s)),
        )
        .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
        .key(key.clone());
        let open_unit = unit.clone();
        let outcome = registry.update(cx, |registry, cx| {
            registry.open_or_focus(key.clone(), cx, move |cx| {
                let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
                    cx.new(|cx| Self::new(open_unit, tab, window, cx))
                })?;
                Ok(handle.into())
            })
        });
        match outcome {
            OpenOutcome::Opened => {}
            OpenOutcome::Focused => {
                let handle = registry
                    .read(cx)
                    .get(&key)
                    .and_then(|handle| handle.downcast::<Root>());
                let view = handle.and_then(|handle| {
                    TlSecondaryWindow::<Self>::find(handle, cx)
                        .map(|frame| (handle, frame.read(cx).content().clone()))
                });
                if let Some((handle, view)) = view {
                    let _ = handle.update(cx, |_, window, cx| {
                        view.update(cx, |this, cx| this.select_tab(tab, window, cx));
                    });
                }
            }
            OpenOutcome::CapReached(_) => {
                let text = tr(cx, "services-sys030-too-many-windows");
                push_main_toast(cx, ToastKind::Warning, text, None, None);
            }
            OpenOutcome::Failed => {
                warn!(target: "tasklens::ui", "service properties window did not open");
            }
        }
    }

    fn new(
        unit: UnitRef,
        tab: ServicePropsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tracker = cx.global::<GlobalActionTracker>().0.clone();
        let actions = cx.new(|cx| SysActions::new(&tracker, cx));
        let mut subs = vec![cx.subscribe_in(
            &tracker,
            window,
            |this, _, event: &ActionTrackerEvent, window, cx| {
                if let ActionTrackerEvent::Finished { id, failed, .. } = *event {
                    this.on_action_finished(id, failed, window, cx);
                }
            },
        )];
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        let mut units: Arc<[UnitRecord]> = Arc::from([]);
        let mut backend = ServiceBackend::Systemd;
        let mut details = None;
        if let Some(hub) = &hub {
            subs.push(cx.subscribe_in(
                hub,
                window,
                |this, hub, event: &SnapshotChanged, window, cx| {
                    if event.mask.intersects(ComponentMask::SERVICES) {
                        let snapshot = hub.read(cx).current().clone();
                        this.take_services(&snapshot.services, window, cx);
                    }
                },
            ));
            let services = hub.read(cx).current().services.clone();
            units = services.units.clone();
            backend = services.backend;
            details = services.details.get(&unit).cloned();
        }
        let not_loaded = unit_not_loaded(&units, &unit);
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let id = cx.entity_id();
            let part = Demand {
                any_surface_visible: true,
                unit_details: Arc::from([unit.clone()]),
                ..Demand::default()
            };
            demand.update(cx, |d, cx| {
                d.set(id, Some(window.window_handle()), part, cx)
            });
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }
        let mut this = Self {
            unit,
            details,
            units,
            backend,
            not_loaded,
            active: ServicePropsTab::General,
            tabs: Default::default(),
            actions,
            applying: SmallVec::new(),
            failed_tab: None,
            close_after_apply: false,
            focus: cx.focus_handle(),
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
            _subs: subs,
        };
        this.select_tab(tab, window, cx);
        // The strip is the first tab stop: focus the root, then step to the first stop.
        this.focus.focus(window, cx);
        window.defer(cx, |window, cx| window.focus_next(cx));
        this
    }

    /// The unit this window shows.
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }

    /// The last details received for the unit. Kept after the unit disappears.
    pub fn details(&self) -> Option<&Arc<UnitDetails>> {
        self.details.as_ref()
    }

    /// True when the unit is gone or not loaded: values are the last seen and actions are off.
    pub fn is_not_loaded(&self) -> bool {
        self.not_loaded
    }

    pub fn active(&self) -> ServicePropsTab {
        self.active
    }

    /// This window's action layer. Tab views submit through it so toasts stay in this window.
    pub fn actions(&self) -> &Entity<SysActions> {
        &self.actions
    }

    /// The tabs shown: all five, or only General under OpenRC.
    pub fn visible_tabs(&self) -> SmallVec<[ServicePropsTab; 5]> {
        match self.backend {
            ServiceBackend::OpenRc => SmallVec::from_slice(&[ServicePropsTab::General]),
            ServiceBackend::Systemd => SmallVec::from_slice(&ALL_TABS),
        }
    }

    /// Apply is on when no apply runs, the unit is loaded, some tab is dirty and every dirty
    /// tab is valid.
    pub fn apply_enabled(&self, cx: &App) -> bool {
        !self.not_loaded
            && self.applying.is_empty()
            && self.any_dirty(cx)
            && self.dirty_tabs_valid(cx)
    }

    fn ok_enabled(&self, cx: &App) -> bool {
        self.applying.is_empty() && self.dirty_tabs_valid(cx)
    }

    fn handles(&self) -> impl Iterator<Item = (ServicePropsTab, &Rc<dyn TabHandle>)> {
        ALL_TABS
            .into_iter()
            .filter_map(|tab| Some((tab, &self.tabs[tab.index()].as_ref()?.handle)))
    }

    fn any_dirty(&self, cx: &App) -> bool {
        self.handles().any(|(_, handle)| handle.is_dirty(cx))
    }

    fn dirty_tabs_valid(&self, cx: &App) -> bool {
        self.handles()
            .all(|(_, handle)| !handle.is_dirty(cx) || handle.is_valid(cx))
    }

    pub fn select_tab(
        &mut self,
        tab: ServicePropsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.visible_tabs().contains(&tab) {
            return;
        }
        self.active = tab;
        if self.tabs[tab.index()].is_none() {
            let input = TabInput {
                unit: self.unit.clone(),
                actions: self.actions.clone(),
                parent: cx.entity_id(),
            };
            let slot = create_tab(tab, input, window, cx);
            if let Some(slot) = &slot {
                slot.handle
                    .set_details(self.details.as_ref(), self.not_loaded, cx);
            }
            self.tabs[tab.index()] = slot;
        }
        cx.notify();
    }

    fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.visible_tabs();
        let Some(at) = visible.iter().position(|tab| *tab == self.active) else {
            return;
        };
        let next = if forward {
            (at + 1) % visible.len()
        } else {
            (at + visible.len() - 1) % visible.len()
        };
        self.select_tab(visible[next], window, cx);
    }

    fn reset_tabs(&self, cx: &mut App) {
        let handles: Vec<_> = self.handles().map(|(_, h)| h.clone()).collect();
        for handle in handles {
            handle.reset(cx);
        }
    }

    /// Applies every dirty tab. With `close`, the window closes once all of them succeeded.
    fn apply_all(&mut self, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.not_loaded {
            if close {
                window.remove_window();
            }
            return;
        }
        let dirty: Vec<_> = self
            .handles()
            .filter(|(_, handle)| handle.is_dirty(cx))
            .map(|(tab, handle)| (tab, handle.clone()))
            .collect();
        self.failed_tab = None;
        for (tab, handle) in dirty {
            let ids = handle.apply(window, cx);
            if ids.is_empty() && handle.is_dirty(cx) {
                // `dirty` is in tab order, so the first rejected tab is the leftmost.
                self.failed_tab.get_or_insert(tab);
            }
            for id in ids {
                self.applying.push((id, tab));
            }
        }
        if self.applying.is_empty() {
            match self.failed_tab.take() {
                None => {
                    self.reset_tabs(cx);
                    if close {
                        window.remove_window();
                    }
                }
                Some(tab) => self.select_tab(tab, window, cx),
            }
        } else {
            self.close_after_apply = close;
        }
        cx.notify();
    }

    fn on_action_finished(
        &mut self,
        id: ActionId,
        failed: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.applying.iter().position(|(sent, _)| *sent == id) else {
            return;
        };
        let (_, tab) = self.applying.remove(at);
        if failed > 0
            && self
                .failed_tab
                .is_none_or(|first| tab.index() < first.index())
        {
            self.failed_tab = Some(tab);
        }
        if !self.applying.is_empty() {
            cx.notify();
            return;
        }
        let close = std::mem::take(&mut self.close_after_apply);
        match self.failed_tab.take() {
            None => {
                self.reset_tabs(cx);
                if close {
                    window.remove_window();
                }
                cx.notify();
            }
            Some(tab) => self.select_tab(tab, window, cx),
        }
    }

    /// Takes a SERVICES change. Notifies only when this unit's details Arc changed, its
    /// "loaded" state changed, or the backend changed.
    fn take_services(
        &mut self,
        services: &ServicesSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(details) = services.details.get(&self.unit)
            && !self
                .details
                .as_ref()
                .is_some_and(|have| Arc::ptr_eq(have, details))
        {
            self.details = Some(details.clone());
            changed = true;
        }
        if !Arc::ptr_eq(&self.units, &services.units) {
            self.units = services.units.clone();
            let not_loaded = unit_not_loaded(&self.units, &self.unit);
            if not_loaded != self.not_loaded {
                self.not_loaded = not_loaded;
                self.retitle(window, cx);
                changed = true;
            }
        }
        if services.backend != self.backend {
            self.backend = services.backend;
            if !self.visible_tabs().contains(&self.active) {
                self.select_tab(ServicePropsTab::General, window, cx);
            }
            changed = true;
        }
        if changed {
            let handles: Vec<_> = self.handles().map(|(_, h)| h.clone()).collect();
            for handle in handles {
                handle.set_details(self.details.as_ref(), self.not_loaded, cx);
            }
            cx.notify();
        }
    }

    fn retitle(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = title_for(&self.unit, self.not_loaded, cx);
        let frame = window.root::<Root>().flatten().and_then(|root| {
            root.read(cx)
                .view()
                .clone()
                .downcast::<TlSecondaryWindow<Self>>()
                .ok()
        });
        if let Some(frame) = frame {
            frame.update(cx, |frame, cx| frame.set_title(title, window, cx));
        } else {
            window.set_window_title(&title);
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn find(handle: gpui_kit::WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    /// How many times `render` ran.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u32 {
        self.renders
    }

    /// The General tab view, once built.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn general_tab(&self) -> Option<Entity<GeneralTab>> {
        let slot = self.tabs[ServicePropsTab::General.index()].as_ref()?;
        slot.view.clone().downcast::<GeneralTab>().ok()
    }

    /// The Log On tab view, once built.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn logon_tab(&self) -> Option<Entity<LogOnTab>> {
        let slot = self.tabs[ServicePropsTab::LogOn.index()].as_ref()?;
        slot.view.clone().downcast::<LogOnTab>().ok()
    }

    /// The Recovery tab view, once built.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn recovery_tab(&self) -> Option<Entity<RecoveryTab>> {
        let slot = self.tabs[ServicePropsTab::Recovery.index()].as_ref()?;
        slot.view.clone().downcast::<RecoveryTab>().ok()
    }

    /// The Dependencies tab view, once built.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn dependencies_tab(&self) -> Option<Entity<DependenciesTab>> {
        let slot = self.tabs[ServicePropsTab::Dependencies.index()].as_ref()?;
        slot.view.clone().downcast::<DependenciesTab>().ok()
    }

    /// Puts `view` in the slot of `tab`, in place of a tab bead's view.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn install_tab_view<T: Render + ServicePropsTabView>(
        &mut self,
        tab: ServicePropsTab,
        view: Entity<T>,
        cx: &mut Context<Self>,
    ) {
        self.tabs[tab.index()] = Some(TabSlot::new(view));
        cx.notify();
    }

    fn footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = metrics(cx);
        let ok_enabled = self.ok_enabled(cx);
        let apply_enabled = self.apply_enabled(cx);
        let this = cx.weak_entity();
        let ok = {
            let this = this.clone();
            TlButton::new("ServicePropsOk", TlButtonKind::Primary)
                .label(tr(cx, "services-sys030-ok"))
                .disabled(!ok_enabled)
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.apply_all(true, window, cx));
                })
        };
        let cancel = TlButton::new("ServicePropsCancel", TlButtonKind::Secondary)
            .label(tr(cx, "services-sys030-cancel"))
            .on_click(|_, window, _| window.remove_window());
        let apply = TlButton::new("ServicePropsApply", TlButtonKind::Secondary)
            .label(tr(cx, "services-sys030-apply"))
            .disabled(!apply_enabled)
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.apply_all(false, window, cx));
            });
        div()
            .debug_selector(|| "ServicePropsFooter".into())
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .gap(m.sp[4])
            .px(m.sp[5])
            .h(px(FOOTER_H * m.ui_scale))
            .bg(theme.toolbar)
            .border_t_1()
            .border_color(theme.divider)
            .child(ok)
            .child(cancel)
            .child(apply)
    }
}

impl Render for ServicePropertiesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let theme = current(cx).clone();
        let m = metrics(cx);
        let visible = self.visible_tabs();
        let specs: Vec<TlTabSpec> = visible
            .iter()
            .map(|tab| TlTabSpec::new(tr(cx, tab.label_key())))
            .collect();
        let selected = visible
            .iter()
            .position(|tab| *tab == self.active)
            .unwrap_or(0);
        let this = cx.weak_entity();
        let strip = TlTabs::new((
            "service-props-tabs",
            window.window_handle().window_id().as_u64(),
        ))
        .tabs(specs)
        .selected(selected)
        .on_change(move |index, window, cx| {
            if let Some(tab) = visible.get(index).copied() {
                let _ = this.update(cx, |this, cx| this.select_tab(tab, window, cx));
            }
        });
        let body = match &self.tabs[self.active.index()] {
            Some(slot) => slot
                .view
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .id("ServicePropertiesWindow")
            .key_context(SERVICE_PROPS_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ServicePropsNextTab, window, cx| {
                this.step_tab(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ServicePropsPrevTab, window, cx| {
                this.step_tab(false, window, cx)
            }))
            .on_action(
                cx.listener(|this, action: &ServicePropsSelectTab, window, cx| {
                    if let Some(tab) = ALL_TABS.get(usize::from(action.index)).copied() {
                        this.select_tab(tab, window, cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &ServicePropsOk, window, cx| {
                if this.ok_enabled(cx) {
                    this.apply_all(true, window, cx);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(div().flex_none().w_full().child(strip))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .p(m.sp[6])
                    .bg(theme.window_bg)
                    .child(body),
            )
            .child(self.footer(cx))
    }
}
