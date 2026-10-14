//! Services filter strip and Unit types menu (UI-SYS-011, UI-SYS-012).

use fluent_bundle::FluentValue;
use gpui_kit::component::h_flex;
use gpui_kit::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, Styled, Window, div, px,
};
use smallvec::SmallVec;
use tl_model::client::EngineClient;
use tl_model::settings::ServicesSettings;
use tl_model::table::{ServiceFilter, ServiceStateFilter, UnitType};

use super::actions::{
    ManagerArg, ServicesRemoveFilters, ServicesResetUnitTypes, ServicesToggleManagerFilter,
    ServicesToggleStateFilter, ServicesToggleUnitType, StateFilterArg, UnitTypeArg,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::menu::TlMenu;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{tr, tr_args};
use crate::pages::sys_table::SysQuery;
use crate::settings_global::TlSettings;
use crate::theme::{current, metrics};

gpui_kit::gpui::actions!(tasklens, [ServicesOpenUnitTypes]);

const ALL_UNIT_TYPES: [UnitTypeArg; 10] = [
    UnitTypeArg::Service,
    UnitTypeArg::Socket,
    UnitTypeArg::Timer,
    UnitTypeArg::Mount,
    UnitTypeArg::Automount,
    UnitTypeArg::Path,
    UnitTypeArg::Target,
    UnitTypeArg::Swap,
    UnitTypeArg::Slice,
    UnitTypeArg::Scope,
];

const DEFAULT_UNIT_TYPES: [UnitTypeArg; 4] = [
    UnitTypeArg::Service,
    UnitTypeArg::Socket,
    UnitTypeArg::Timer,
    UnitTypeArg::Mount,
];

/// Session state of the Services filter strip.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceFilterState {
    /// Empty means every state.
    pub states: SmallVec<[StateFilterArg; 4]>,
    /// Never empty.
    pub unit_types: SmallVec<[UnitTypeArg; 10]>,
    pub system: bool,
    pub user: bool,
}

impl ServiceFilterState {
    pub fn from_settings(settings: &ServicesSettings) -> Self {
        let states = settings
            .filter_states
            .iter()
            .map(state_arg)
            .collect::<SmallVec<_>>();
        let mut unit_types = settings
            .unit_types
            .iter()
            .map(unit_type_arg)
            .collect::<SmallVec<_>>();
        if unit_types.is_empty() {
            unit_types.extend(DEFAULT_UNIT_TYPES);
        }
        Self {
            states,
            unit_types,
            system: true,
            user: true,
        }
    }

    pub fn is_default(&self) -> bool {
        self.states.is_empty()
            && self.unit_types.len() == DEFAULT_UNIT_TYPES.len()
            && DEFAULT_UNIT_TYPES
                .iter()
                .all(|kind| self.unit_types.contains(kind))
            && self.system
            && self.user
    }

    pub fn to_filter(&self) -> ServiceFilter {
        ServiceFilter {
            states: self.states.iter().map(service_state).collect(),
            unit_types: self.unit_types.iter().map(unit_type).collect(),
            system: self.system,
            user: self.user,
        }
    }

    pub fn toggle_state(&mut self, state: StateFilterArg) {
        toggle_member(&mut self.states, state);
    }

    pub fn toggle_manager(&mut self, manager: ManagerArg) {
        match manager {
            ManagerArg::System => self.system = !self.system,
            ManagerArg::User => self.user = !self.user,
        }
    }

    /// Returns `false` without changing state when this would remove the final unit type.
    pub fn toggle_unit_type(&mut self, kind: UnitTypeArg) -> bool {
        if let Some(ix) = self.unit_types.iter().position(|item| item == &kind) {
            if self.unit_types.len() == 1 {
                return false;
            }
            self.unit_types.remove(ix);
        } else {
            self.unit_types.push(kind);
        }
        true
    }

    pub fn reset_unit_types(&mut self) {
        self.unit_types = SmallVec::from_vec(DEFAULT_UNIT_TYPES.to_vec());
    }

    /// Restores strip defaults. The text filter belongs to the shared table query and is untouched.
    pub fn remove_filters(&mut self) {
        self.states.clear();
        self.reset_unit_types();
        self.system = true;
        self.user = true;
    }
}

impl Default for ServiceFilterState {
    fn default() -> Self {
        Self::from_settings(&ServicesSettings::default())
    }
}

/// Renders the 28 px strip. The Services page handles the dispatched actions.
pub fn render_strip(
    state: &ServiceFilterState,
    disabled: bool,
    menu_open: bool,
    openrc: bool,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = current(cx).clone();
    let m = *metrics(cx);
    let separator = || {
        div()
            .debug_selector(|| "ServicesFilterStrip-separator".into())
            .flex_none()
            .w(px(1.))
            .h(px(16. * m.ui_scale))
            .bg(theme.divider)
    };
    let segment = |id, label, on, action| {
        TlButton::new(id, TlButtonKind::Segment { on })
            .size(TlButtonSize::Sm)
            .label(tr(cx, label))
            .action(action)
            .disabled(disabled)
    };

    let mut strip = h_flex()
        .debug_selector(|| "ServicesFilterStrip".into())
        .w_full()
        .h(px(28. * m.ui_scale))
        .px(m.sp[4])
        .gap(m.sp[3])
        .bg(theme.toolbar)
        .border_b_1()
        .border_color(theme.divider)
        .child(segment(
            "services-filter-running",
            "services-sys011-running",
            state.states.contains(&StateFilterArg::Running),
            Box::new(ServicesToggleStateFilter {
                state: StateFilterArg::Running,
            }),
        ))
        .child(segment(
            "services-filter-failed",
            "services-sys011-failed",
            state.states.contains(&StateFilterArg::Failed),
            Box::new(ServicesToggleStateFilter {
                state: StateFilterArg::Failed,
            }),
        ))
        .child(segment(
            "services-filter-stopped",
            "services-sys011-stopped",
            state.states.contains(&StateFilterArg::Stopped),
            Box::new(ServicesToggleStateFilter {
                state: StateFilterArg::Stopped,
            }),
        ))
        .child(segment(
            "services-filter-disabled",
            "services-sys011-disabled",
            state.states.contains(&StateFilterArg::Disabled),
            Box::new(ServicesToggleStateFilter {
                state: StateFilterArg::Disabled,
            }),
        ));

    if !openrc {
        let unit_types = if state.unit_types.len() == ALL_UNIT_TYPES.len() {
            tr(cx, "services-sys011-unit-types")
        } else {
            tr_args(
                cx,
                "services-sys011-unit-types-count",
                &[("count", FluentValue::from(state.unit_types.len() as u64))],
            )
        };
        strip = strip
            .child(separator())
            .child(
                TlButton::new("services-filter-unit-types", TlButtonKind::Ghost)
                    .size(TlButtonSize::Sm)
                    .label(unit_types)
                    .trailing_icon(TlIcon::ChevronDown)
                    .icon_size(m.icon.xs)
                    .action(Box::new(ServicesOpenUnitTypes))
                    .pressed(menu_open)
                    .disabled(disabled),
            )
            .child(separator())
            .child(segment(
                "services-filter-system",
                "services-sys011-system",
                state.system,
                Box::new(ServicesToggleManagerFilter {
                    manager: ManagerArg::System,
                }),
            ))
            .child(segment(
                "services-filter-user",
                "services-sys011-user",
                state.user,
                Box::new(ServicesToggleManagerFilter {
                    manager: ManagerArg::User,
                }),
            ));
    }

    strip
        .child(separator())
        .child(
            TlButton::new("services-filter-remove", TlButtonKind::Ghost)
                .size(TlButtonSize::Sm)
                .label(tr(cx, "services-sys011-remove-filters"))
                .action(Box::new(ServicesRemoveFilters))
                .tooltip(tr(cx, "services-sys011-show-all-units"))
                .disabled(disabled || state.is_default()),
        )
        .into_any_element()
}

/// Adds the ten keep-open check items and Reset to default to a `TlMenu`.
pub fn unit_types_menu(
    state: &ServiceFilterState,
    mut menu: TlMenu,
    _window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let last_checked = state.unit_types.len() == 1;
    for kind in ALL_UNIT_TYPES {
        let checked = state.unit_types.contains(&kind);
        menu = menu.check_item(
            tr(cx, unit_type_label(&kind)),
            checked,
            Box::new(ServicesToggleUnitType { kind }),
        );
        // The menu flips a kept-open mark by itself. The last checked type is refused, so its
        // mark must not flip: that row closes the menu and keeps the mark instead.
        if !(checked && last_checked) {
            menu = menu.keep_open();
        }
    }
    menu.separator().item(
        tr(cx, "services-sys012-reset-to-default"),
        Box::new(ServicesResetUnitTypes),
    )
}

/// Sends the strip's filter at once, with no debounce.
fn send_filter(state: &ServiceFilterState, query: &mut SysQuery, client: &dyn EngineClient) {
    query.set_services_filter(state.to_filter(), client);
}

/// Writes states and unit types to `settings.toml`. System and User are never persisted.
fn persist(state: &ServiceFilterState, cx: &mut App) {
    let filter = state.to_filter();
    TlSettings::update(cx, |data| {
        data.settings.services.filter_states = filter.states.into_vec();
        data.settings.services.unit_types = filter.unit_types.into_vec();
    });
}

/// Handler for `ServicesToggleStateFilter`: send the query, then persist.
pub fn apply_toggle_state(
    state: &mut ServiceFilterState,
    arg: StateFilterArg,
    query: &mut SysQuery,
    client: &dyn EngineClient,
    cx: &mut App,
) {
    state.toggle_state(arg);
    send_filter(state, query, client);
    persist(state, cx);
}

/// Handler for `ServicesToggleManagerFilter`: send the query. Nothing is persisted.
pub fn apply_toggle_manager(
    state: &mut ServiceFilterState,
    manager: ManagerArg,
    query: &mut SysQuery,
    client: &dyn EngineClient,
) {
    state.toggle_manager(manager);
    send_filter(state, query, client);
}

/// Handler for `ServicesToggleUnitType`. Refusing the last checked type sends nothing, writes
/// nothing and shows the Warning toast. Returns whether the toggle was applied.
pub fn apply_toggle_unit_type(
    state: &mut ServiceFilterState,
    kind: UnitTypeArg,
    query: &mut SysQuery,
    client: &dyn EngineClient,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !state.toggle_unit_type(kind) {
        let text = tr(cx, "services-sys012-at-least-one-unit-type");
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |queue, cx| {
                queue.push(ToastKind::Warning, text, None, None, cx);
            });
        }
        return false;
    }
    send_filter(state, query, client);
    persist(state, cx);
    true
}

/// Handler for `ServicesResetUnitTypes`.
pub fn apply_reset_unit_types(
    state: &mut ServiceFilterState,
    query: &mut SysQuery,
    client: &dyn EngineClient,
    cx: &mut App,
) {
    state.reset_unit_types();
    send_filter(state, query, client);
    persist(state, cx);
}

/// Handler for `ServicesRemoveFilters`. The text filter lives in `query` and is kept.
pub fn apply_remove_filters(
    state: &mut ServiceFilterState,
    query: &mut SysQuery,
    client: &dyn EngineClient,
    cx: &mut App,
) {
    state.remove_filters();
    send_filter(state, query, client);
    persist(state, cx);
}

fn toggle_member<T: PartialEq>(items: &mut SmallVec<[T; 4]>, item: T) {
    if let Some(ix) = items.iter().position(|candidate| candidate == &item) {
        items.remove(ix);
    } else {
        items.push(item);
    }
}

fn state_arg(state: &ServiceStateFilter) -> StateFilterArg {
    match state {
        ServiceStateFilter::Running => StateFilterArg::Running,
        ServiceStateFilter::Failed => StateFilterArg::Failed,
        ServiceStateFilter::Stopped => StateFilterArg::Stopped,
        ServiceStateFilter::Disabled => StateFilterArg::Disabled,
    }
}

fn service_state(state: &StateFilterArg) -> ServiceStateFilter {
    match state {
        StateFilterArg::Running => ServiceStateFilter::Running,
        StateFilterArg::Failed => ServiceStateFilter::Failed,
        StateFilterArg::Stopped => ServiceStateFilter::Stopped,
        StateFilterArg::Disabled => ServiceStateFilter::Disabled,
    }
}

fn unit_type_arg(kind: &UnitType) -> UnitTypeArg {
    match kind {
        UnitType::Service => UnitTypeArg::Service,
        UnitType::Socket => UnitTypeArg::Socket,
        UnitType::Timer => UnitTypeArg::Timer,
        UnitType::Mount => UnitTypeArg::Mount,
        UnitType::Automount => UnitTypeArg::Automount,
        UnitType::Path => UnitTypeArg::Path,
        UnitType::Target => UnitTypeArg::Target,
        UnitType::Swap => UnitTypeArg::Swap,
        UnitType::Slice => UnitTypeArg::Slice,
        UnitType::Scope => UnitTypeArg::Scope,
    }
}

fn unit_type(kind: &UnitTypeArg) -> UnitType {
    match kind {
        UnitTypeArg::Service => UnitType::Service,
        UnitTypeArg::Socket => UnitType::Socket,
        UnitTypeArg::Timer => UnitType::Timer,
        UnitTypeArg::Mount => UnitType::Mount,
        UnitTypeArg::Automount => UnitType::Automount,
        UnitTypeArg::Path => UnitType::Path,
        UnitTypeArg::Target => UnitType::Target,
        UnitTypeArg::Swap => UnitType::Swap,
        UnitTypeArg::Slice => UnitType::Slice,
        UnitTypeArg::Scope => UnitType::Scope,
    }
}

fn unit_type_label(kind: &UnitTypeArg) -> &'static str {
    match kind {
        UnitTypeArg::Service => "services-sys012-services",
        UnitTypeArg::Socket => "services-sys012-sockets",
        UnitTypeArg::Timer => "services-sys012-timers",
        UnitTypeArg::Mount => "services-sys012-mounts",
        UnitTypeArg::Automount => "services-sys012-automounts",
        UnitTypeArg::Path => "services-sys012-paths",
        UnitTypeArg::Target => "services-sys012-targets",
        UnitTypeArg::Swap => "services-sys012-swaps",
        UnitTypeArg::Slice => "services-sys012-slices",
        UnitTypeArg::Scope => "services-sys012-scopes",
    }
}
