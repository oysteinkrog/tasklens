//! The "tasklens Settings" window: frame, page list and `PageBuilder` (E21-018, UI-SHL-060,
//! APP-023, APP-045, R-028, UI-FND-021).
//!
//! A Tool window (900 × 640, minimum 720 × 480, parent = main window, not modal) holding the
//! kit `Settings` widget with a 220 px sidebar, its search field and the page list. Pages come
//! from [`all_pages`] in page order; a page with no item in the current build is not listed.
//! The page beads (E21-019 to E21-026, E21-A2) add one `mod` line and one call there.
//!
//! Each page is built with [`PageBuilder`]: one method per control kind, each binding one
//! setting through the kit `SettingField` get and set closures and recording the key, so a test
//! can check that every key is bound once. Writes go through `TlSettings` (`set` and `update`
//! persist; a slider previews on Change and persists on Release).
//!
//! The window re-renders on input, on `TlSettings` changes and on the device catalog
//! (`ComponentMask::DEVICES`, for the Performance page lists) and on `HEALTH` (the Privileges
//! helper status line), never on other snapshots. Slider
//! states are created in `SettingsWindow::new`, never in render.

mod advanced;
mod appearance;
mod general;
mod graphs;
mod motion;
mod performance;
mod privileges;
mod processes;
mod services;
mod startup_window;
mod units;

pub(crate) use performance::{set_fan_label, set_fan_temp};

use std::cell::RefCell;
use std::ops::RangeInclusive;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::slider::SliderState;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
    Settings as KitSettings,
};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::text::Text;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, ElementId, Entity, FontWeight, Global,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px, size,
};
use tracing::warn;

use super::main::ShellState;
use super::registry::{GlobalRegistry, WindowKey};
use crate::actions::OpenSettings;
use crate::components::banner::{TlBanner, TlBannerKind};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::checkbox::TlCheckbox;
use crate::components::select::{TlSelect, TlSelectItem, TlSelectState};
use crate::components::slider::{TlSlider, TlSliderEvent};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::text::style::t_sm;
use crate::theme::{current, metrics};

/// Window size and minimum size before the UI scale.
pub const SIZE: (f32, f32) = (900., 640.);
pub const MIN_SIZE: (f32, f32) = (720., 480.);
/// Sidebar width before the UI scale.
pub const SIDEBAR_W: f32 = 220.;
/// Space kept free at the right of every group so the kit scrollbar does not cover controls,
/// before the UI scale.
pub const SCROLLBAR_GAP: f32 = 12.;
/// A page needs this many groups to get sub-entries (and a chevron) in the sidebar.
const MIN_SIDEBAR_GROUPS: usize = 3;

const TITLE: &str = "shell-settings-title";
const RESTART_NOTE: &str = "shell-settings-restart-note";

/// Registers the `OpenSettings` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    advanced::init(cx);
    // Deferred: from a menu the main window is mid-update.
    cx.on_action(|_: &OpenSettings, cx| cx.defer(open_settings));
}

/// Focuses the open Settings window, or opens it. Without a window registry (tests) it always
/// opens one.
pub fn open_settings(cx: &mut App) {
    let s = metrics(cx).ui_scale;
    let mut spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(WindowKey::Settings);
    if let Some(main) = cx.try_global::<ShellState>().and_then(|s| s.main_window) {
        spec = spec.parent(main.into());
    }
    let open = move |cx: &mut App| {
        TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| SettingsWindow::new(window, cx))
        })
        .map(Into::into)
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Settings, cx, open));
        }
        None => {
            if let Err(err) = open(cx) {
                warn!(target: "tasklens::ui", %err, "Settings window did not open");
            }
        }
    }
}

/// The pages in sidebar order (UI-SHL-060).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageId {
    General,
    Appearance,
    Motion,
    Units,
    Processes,
    Performance,
    Graphs,
    Services,
    StartupWindow,
    Privileges,
    Advanced,
}

impl PageId {
    pub const ALL: [PageId; 11] = [
        Self::General,
        Self::Appearance,
        Self::Motion,
        Self::Units,
        Self::Processes,
        Self::Performance,
        Self::Graphs,
        Self::Services,
        Self::StartupWindow,
        Self::Privileges,
        Self::Advanced,
    ];

    fn title_key(self) -> &'static str {
        match self {
            Self::General => "shell-settings-page-general",
            Self::Appearance => "shell-settings-page-appearance",
            Self::Motion => "shell-settings-page-motion",
            Self::Units => "shell-settings-page-units",
            Self::Processes => "shell-settings-page-processes",
            Self::Performance => "shell-settings-page-performance",
            Self::Graphs => "shell-settings-page-graphs",
            Self::Services => "shell-settings-page-services",
            Self::StartupWindow => "shell-settings-page-startup-window",
            Self::Privileges => "shell-settings-page-privileges",
            Self::Advanced => "shell-settings-page-advanced",
        }
    }

    /// The Lucide icon of the page list row, drawn at 16 px.
    pub fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings,
            Self::Appearance => IconName::Palette,
            Self::Motion => IconName::Activity,
            Self::Units => IconName::Scaling,
            Self::Processes => IconName::ListTree,
            Self::Performance => IconName::Gauge,
            Self::Graphs => IconName::ChartArea,
            Self::Services => IconName::Server,
            Self::StartupWindow => IconName::AppWindow,
            Self::Privileges => IconName::Shield,
            Self::Advanced => IconName::SlidersHorizontal,
        }
    }
}

/// Every page in page order, each from its page module's `page(cx)`. A page bead adds one
/// `mod` line above and one call here.
pub fn all_pages(cx: &mut App) -> Vec<PageBuilder> {
    // Page order: General, Appearance, Motion, Units, Processes, Performance, Graphs,
    // Services, Startup and window, Privileges, Advanced.
    #[allow(unused_mut, reason = "test-hooks appends the test pages")]
    let mut pages: Vec<PageBuilder> = vec![
        general::page(cx),
        appearance::page(cx),
        motion::page(cx),
        units::page(cx),
        processes::page(cx),
        performance::page(cx),
        graphs::page(cx),
        services::page(cx),
        startup_window::page(cx),
        privileges::page(cx),
        advanced::page(cx),
    ];
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(test) = cx.try_global::<TestPages>().map(|t| t.0) {
        pages.extend(test(cx));
    }
    pages
}

/// Extra pages for tests, appended after the real ones.
#[cfg(any(test, feature = "test-hooks"))]
pub struct TestPages(pub fn(&mut App) -> Vec<PageBuilder>);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for TestPages {}

/// A slider item's range and step, so `SettingsWindow::new` can create its state.
#[derive(Clone)]
struct SliderSpec {
    key: SettingKey,
    range: RangeInclusive<i64>,
    step: i64,
    on_commit: Option<SliderCommit>,
}

/// The slider states of the open Settings window, by key. Render looks them up here.
#[derive(Default)]
struct SliderStates(Vec<(SettingKey, Entity<SliderState>)>);

impl Global for SliderStates {}

impl SliderStates {
    fn get(&self, key: SettingKey) -> Option<&Entity<SliderState>> {
        self.0.iter().find(|(k, _)| *k == key).map(|(_, s)| s)
    }
}

/// `v` clamped to `min..=max`, snapped to the nearest `step` counted from `min`.
fn snap_number(v: f64, min: i64, max: i64, step: i64) -> i64 {
    let (lo, hi, step) = (min as f64, max as f64, step.max(1) as f64);
    let v = v.clamp(lo, hi);
    let snapped = lo + ((v - lo) / step).round() * step;
    snapped.clamp(lo, hi).round() as i64
}

type CustomRender = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type BoolSetter = Rc<dyn Fn(bool, &mut App)>;
/// A number input setter taking the typed value.
type NumberSetter = Rc<dyn Fn(f64, &mut App)>;
type BoolGetter = Rc<dyn Fn(&App) -> bool>;
/// Runs with the value after a slider's Release write (the UI Scale toast).
pub type SliderCommit = fn(i64, &mut App);
/// What picking a dropdown option does with its value.
type PickValue = Rc<dyn Fn(&SettingValue, &mut App)>;
/// A dropdown setter taking the index of the picked option.
type PickSetter = Rc<dyn Fn(usize, &mut App)>;
/// A checklist section as rendered: its heading and `(label, value, setter)` per checkbox.
type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type CheckRow = (SharedString, Vec<(SharedString, SharedString, BoolSetter)>);

/// Builds one page: groups of items, each control bound to one setting. `keys` records every
/// key a control binds, in order.
pub struct PageBuilder {
    id: PageId,
    groups: Vec<(SharedString, Vec<SettingItem>)>,
    keys: Vec<SettingKey>,
    sliders: Vec<SliderSpec>,
    /// "Takes effect after restart": the next item gets it as description and as a toast.
    restart_next: Option<SharedString>,
    /// The next item is disabled.
    disable_next: bool,
    /// The next switch's left indent, in px before the UI scale.
    indent_next: f32,
    /// The next switch's tooltip.
    tooltip_next: Option<SharedString>,
    /// The next item's restart toast when it is not the restart note itself.
    restart_toast_next: Option<SharedString>,
    /// Runs after the next slider's Release write.
    commit_next: Option<SliderCommit>,
    description: Option<SharedString>,
    /// The switch and checkbox setters, the same closures the kit calls.
    #[cfg(any(test, feature = "test-hooks"))]
    bool_setters: Vec<(SettingKey, BoolSetter)>,
    /// The number input setters, the same closures the kit calls.
    #[cfg(any(test, feature = "test-hooks"))]
    numbers: Vec<(SettingKey, NumberSetter)>,
    /// The dropdown option labels and setters, the same closure the kit calls.
    #[cfg(any(test, feature = "test-hooks"))]
    dropdowns: Vec<(SettingKey, Vec<SharedString>, PickSetter)>,
    /// The description each item shows.
    #[cfg(any(test, feature = "test-hooks"))]
    descriptions: Vec<(SettingKey, SharedString)>,
    /// The keys of the disabled items.
    #[cfg(any(test, feature = "test-hooks"))]
    disabled: Vec<SettingKey>,
    /// The banner texts, top first.
    #[cfg(any(test, feature = "test-hooks"))]
    banners: Vec<SharedString>,
    /// The checklist setters by key and value, the same closures the checkboxes call.
    #[cfg(any(test, feature = "test-hooks"))]
    checks: Vec<(SettingKey, SharedString, BoolSetter)>,
    /// The empty texts the checklists show.
    #[cfg(any(test, feature = "test-hooks"))]
    empty_texts: Vec<(SettingKey, SharedString)>,
    /// The button labels and actions.
    #[cfg(any(test, feature = "test-hooks"))]
    buttons: Vec<(SharedString, Rc<dyn Action>)>,
    /// The keyless switches by label.
    #[cfg(any(test, feature = "test-hooks"))]
    locals: Vec<(SharedString, LocalSwitch)>,
    /// The click handlers of the callback buttons by label.
    #[cfg(any(test, feature = "test-hooks"))]
    button_fns: Vec<(SharedString, ClickHandler)>,
    /// The custom item labels and their render closures.
    #[cfg(any(test, feature = "test-hooks"))]
    customs: Vec<(SharedString, CustomRender)>,
    /// The indent of each indented switch, in px before the UI scale.
    #[cfg(any(test, feature = "test-hooks"))]
    indents: Vec<(SettingKey, f32)>,
    /// The tooltip of each switch that has one.
    #[cfg(any(test, feature = "test-hooks"))]
    tooltips: Vec<(SettingKey, SharedString)>,
}

/// A switch bound to no setting (Open with Ctrl+Esc). The owner keeps its value; `set` does not
/// write settings. The page redraws after `set` runs, so `get` and `description` show the
/// new state.
#[derive(Clone)]
pub struct LocalSwitch {
    pub get: Rc<dyn Fn() -> bool>,
    pub description: Rc<dyn Fn() -> Option<SharedString>>,
    pub set: BoolSetter,
    pub disabled: bool,
}

/// One checklist section: a sub-heading (none when empty) and its `(label, value)` checkboxes.
pub struct CheckSection {
    pub heading: SharedString,
    pub options: Vec<(SharedString, SharedString)>,
}

impl PageBuilder {
    pub fn new(id: PageId) -> Self {
        Self {
            id,
            groups: Vec::new(),
            keys: Vec::new(),
            sliders: Vec::new(),
            restart_next: None,
            disable_next: false,
            indent_next: 0.,
            tooltip_next: None,
            restart_toast_next: None,
            commit_next: None,
            description: None,
            #[cfg(any(test, feature = "test-hooks"))]
            bool_setters: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            numbers: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            dropdowns: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            descriptions: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            disabled: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            banners: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            checks: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            empty_texts: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            buttons: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            locals: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            button_fns: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            customs: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            indents: Vec::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            tooltips: Vec::new(),
        }
    }

    pub fn id(&self) -> PageId {
        self.id
    }

    /// Every key the page's controls bind, in the order they were added.
    pub fn keys(&self) -> &[SettingKey] {
        &self.keys
    }

    /// True when the page has no item, so it is not listed.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// Marks the next item as needing a restart (`appearance.decorations`, `rendering.gpu`): it
    /// gets the description "Takes effect after restart" and pushes that text as an Info toast
    /// when it changes.
    pub fn restart_note(&mut self, cx: &App) -> &mut Self {
        self.restart_next = Some(tr(cx, RESTART_NOTE));
        self
    }

    /// Like [`Self::restart_note`], but the toast says `toast` instead of the note
    /// (`appearance.decorations`: "Restart tasklens to apply the title bar change").
    pub fn restart_note_toast(&mut self, cx: &App, toast: SharedString) -> &mut Self {
        self.restart_next = Some(tr(cx, RESTART_NOTE));
        self.restart_toast_next = Some(toast);
        self
    }

    /// The text under the page title.
    pub fn page_description(&mut self, text: impl Into<SharedString>) -> &mut Self {
        self.description = Some(text.into());
        self
    }

    /// Disables the next item when `disabled` is true. A disabled slider is skipped by Tab.
    pub fn disable_next(&mut self, disabled: bool) -> &mut Self {
        self.disable_next = disabled;
        self
    }

    /// Indents the next switch by `indent` px (before the UI scale), for a row that depends on
    /// the row above it.
    pub fn indent_next(&mut self, indent: f32) -> &mut Self {
        self.indent_next = indent;
        self
    }

    /// Gives the next switch a tooltip when `text` is set (why a disabled row is disabled).
    pub fn tooltip_next(&mut self, text: Option<SharedString>) -> &mut Self {
        self.tooltip_next = text;
        self
    }

    /// Runs `on_commit` with the value after the next slider's Release write.
    pub fn slider_on_commit(&mut self, on_commit: SliderCommit) -> &mut Self {
        self.commit_next = Some(on_commit);
        self
    }

    /// A full-width `TlBanner` row in `group`.
    pub fn banner(
        &mut self,
        group: impl Into<SharedString>,
        kind: TlBannerKind,
        text: SharedString,
    ) -> &mut Self {
        #[cfg(any(test, feature = "test-hooks"))]
        self.banners.push(text.clone());
        let item = SettingItem::render(move |_, _, _| TlBanner::new(kind, text.clone()));
        self.push_item(group, item);
        self
    }

    /// Checkboxes over the members of the StrList `key`, in `sections`. A checkbox is checked
    /// while its value is in the list, or while it is not when `inverted` (a hidden list).
    /// Checking adds the value, unchecking removes it (the other way round when `inverted`).
    /// With no option at all the item shows `empty`.
    pub fn checklist(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        sections: Vec<CheckSection>,
        inverted: bool,
        empty: SharedString,
    ) -> &mut Self {
        let restart = self.take_restart();
        let setter = |value: SharedString| -> BoolSetter {
            Rc::new(move |on, cx| set_member(cx, key, &value, on != inverted))
        };
        let sections: Rc<[CheckRow]> = sections
            .into_iter()
            .map(|section| {
                let options = section
                    .options
                    .into_iter()
                    .map(|(label, value)| {
                        let set = setter(value.clone());
                        (label, value, set)
                    })
                    .collect();
                (section.heading, options)
            })
            .collect();
        #[cfg(any(test, feature = "test-hooks"))]
        {
            for (_, options) in sections.iter() {
                for (_, value, set) in options {
                    self.checks.push((key, value.clone(), set.clone()));
                }
            }
            if sections.iter().all(|(_, options)| options.is_empty()) {
                self.empty_texts.push((key, empty.clone()));
            }
        }
        let field = SettingField::render(move |_, _, cx| {
            let theme = current(cx).clone();
            let m = *metrics(cx);
            if sections.iter().all(|(_, options)| options.is_empty()) {
                let mut text = div().text_color(theme.text_secondary).child(empty.clone());
                *text.text_style() = t_sm(&m);
                return text.into_any_element();
            }
            let members = read_list(cx, key);
            let mut list = div().flex().flex_col().gap(m.sp[2]);
            for (i, (heading, options)) in sections.iter().enumerate() {
                if !heading.is_empty() {
                    let mut h = div()
                        .text_color(theme.text_secondary)
                        .child(heading.clone());
                    *h.text_style() = t_sm(&m);
                    list = list.child(h);
                }
                let mut row = div().flex().flex_row().flex_wrap().gap(m.sp[4]);
                for (label, value, set) in options {
                    let present = members.iter().any(|v| v == value.as_ref());
                    let set = set.clone();
                    row = row.child(
                        TlCheckbox::new(SharedString::from(format!(
                            "{}-{i}-{value}",
                            key.as_str()
                        )))
                        .label(label.clone())
                        .checked(Some(present != inverted))
                        .on_change(move |on, _, cx| set(on, cx)),
                    );
                }
                list = list.child(row);
            }
            list.into_any_element()
        });
        self.push(group, key, label, None, restart, field)
    }

    /// Checkboxes over the members of the StrList `key`, in `columns` side by side, each read
    /// top to bottom, column 1 first. A checkbox is checked while its value is in the list.
    /// The stored list keeps the option order. With `keep_one`, unchecking the last checked
    /// option is refused: the box stays checked and `keep_one` shows as an Info toast.
    pub fn check_columns(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        columns: Vec<Vec<(SharedString, SharedString)>>,
        keep_one: Option<SharedString>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let order: Rc<[SharedString]> = columns
            .iter()
            .flatten()
            .map(|(_, value)| value.clone())
            .collect();
        let columns: Rc<[Vec<(SharedString, SharedString, BoolSetter)>]> = columns
            .into_iter()
            .map(|options| {
                options
                    .into_iter()
                    .map(|(label, value)| {
                        let (order, keep_one, v) = (order.clone(), keep_one.clone(), value.clone());
                        let set: BoolSetter = Rc::new(move |on, cx| {
                            set_member_ordered(cx, key, &v, on, &order, keep_one.as_ref())
                        });
                        (label, value, set)
                    })
                    .collect()
            })
            .collect();
        #[cfg(any(test, feature = "test-hooks"))]
        for (_, value, set) in columns.iter().flatten() {
            self.checks.push((key, value.clone(), set.clone()));
        }
        let field = SettingField::render(move |_, _, cx| {
            let m = *metrics(cx);
            let members = read_list(cx, key);
            let mut row = div().flex().flex_row().gap(m.sp[6]);
            for (i, options) in columns.iter().enumerate() {
                let mut column = div().flex().flex_col().gap(m.sp[2]);
                for (label, value, set) in options {
                    let present = members.iter().any(|v| v == value.as_ref());
                    let set = set.clone();
                    column = column.child(
                        TlCheckbox::new(SharedString::from(format!(
                            "{}-{i}-{value}",
                            key.as_str()
                        )))
                        .label(label.clone())
                        .checked(Some(present))
                        .on_change(move |on, _, cx| set(on, cx)),
                    );
                }
                row = row.child(column);
            }
            row.into_any_element()
        });
        self.push(group, key, label, None, restart, field)
    }

    /// Sets the description of the last item.
    pub fn description(&mut self, text: impl Into<SharedString>) -> &mut Self {
        let text = text.into();
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(key) = self.keys.last() {
            self.descriptions.push((*key, text.clone()));
        }
        if let Some(item) = self.groups.last_mut().and_then(|(_, items)| items.pop()) {
            let item = item.description(text);
            self.push_item_raw(item);
        }
        self
    }

    /// Adds search keywords to the last item.
    pub fn keywords(&mut self, words: &[&'static str]) -> &mut Self {
        if let Some(item) = self.groups.last_mut().and_then(|(_, items)| items.pop()) {
            let item = item.keywords(words.iter().copied());
            self.push_item_raw(item);
        }
        self
    }

    pub fn switch(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let setter = self.bool_setter(key, restart.clone());
        let get: BoolGetter = Rc::new(move |cx| read_bool(cx, key));
        self.push_switch(group, key, label, description, restart, get, setter)
    }

    /// A switch that shows `key` and, when switched to the other value, dispatches `action()`
    /// instead of writing. The action's handler writes the key (Always On Top, Start at login,
    /// the Public IP notice).
    pub fn action_switch(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        action: fn() -> Box<dyn Action>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let setter: BoolSetter = Rc::new(move |on, cx| {
            if on != read_bool(cx, key) {
                dispatch_deferred(action(), cx);
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.bool_setters.push((key, setter.clone()));
        let get: BoolGetter = Rc::new(move |cx| read_bool(cx, key));
        self.push_switch(group, key, label, None, restart, get, setter)
    }

    /// A switch over the Str `key`: on stores `on`, off stores `off`. Any other stored value
    /// shows as off.
    pub fn str_switch(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        on: &'static str,
        off: &'static str,
    ) -> &mut Self {
        let restart = self.take_restart();
        let toast = self.take_toast(&restart);
        let setter: BoolSetter = Rc::new(move |checked, cx| {
            let value = if checked { on } else { off };
            TlSettings::set(cx, key, SettingValue::Str(value.into()));
            if let Some(note) = &toast {
                info_toast(note.clone(), cx);
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.bool_setters.push((key, setter.clone()));
        let get: BoolGetter = Rc::new(move |cx| read(cx, key).as_str() == Some(on));
        self.push_switch(group, key, label, None, restart, get, setter)
    }

    pub fn checkbox(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let setter = self.bool_setter(key, restart.clone());
        let field =
            SettingField::checkbox(move |cx| read_bool(cx, key), move |on, cx| setter(on, cx));
        self.push(group, key, label, description, restart, field)
    }

    /// A dropdown of `(label, stored value)` options. A stored value that is none of them shows
    /// no selection.
    pub fn dropdown(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        options: &[(SharedString, SettingValue)],
    ) -> &mut Self {
        let restart = self.take_restart();
        let toast = self.take_toast(&restart);
        let write: PickValue = Rc::new(move |value, cx| {
            TlSettings::set(cx, key, value.clone());
            if let Some(note) = &toast {
                info_toast(note.clone(), cx);
            }
        });
        self.dropdown_with(group, key, label, options, restart, write)
    }

    /// A dropdown of `(label, stored value)` options that shows `key` and, on a pick, dispatches
    /// `action(value)` instead of writing (the Theme dropdown's `ActivateTheme`).
    pub fn action_dropdown(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        options: &[(SharedString, SettingValue)],
        action: fn(&str) -> Box<dyn Action>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let send: PickValue = Rc::new(move |value, cx| {
            if let SettingValue::Str(value) = value {
                dispatch_deferred(action(value), cx);
            }
        });
        self.dropdown_with(group, key, label, options, restart, send)
    }

    fn dropdown_with(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        options: &[(SharedString, SettingValue)],
        restart: Option<SharedString>,
        on_pick: PickValue,
    ) -> &mut Self {
        let values: Rc<[SettingValue]> = options.iter().map(|(_, v)| v.clone()).collect();
        let items: Vec<TlSelectItem> = options
            .iter()
            .enumerate()
            .map(|(i, (label, _))| TlSelectItem::new(i.to_string(), label.clone()))
            .collect();
        let read_values = values.clone();
        let pick: PickSetter = Rc::new(move |i, cx| {
            if let Some(value) = values.get(i) {
                on_pick(value, cx);
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.dropdowns.push((
            key,
            options.iter().map(|(label, _)| label.clone()).collect(),
            pick.clone(),
        ));
        // A `TlSelect`, not the kit dropdown: its popup is opaque over its whole box.
        let id = SharedString::from(format!("settings-select-{}", key.as_str()));
        let field = SettingField::render(move |_, window, cx| {
            let state = window.use_keyed_state(ElementId::Name(id.clone()), cx, |_, cx| {
                TlSelectState::new(items.clone(), None, cx)
            });
            let now = read(cx, key);
            let selected = read_values
                .iter()
                .position(|v| value_matches(v, &now))
                .map(|i| i.to_string())
                .unwrap_or_default();
            state.update(cx, |st, cx| {
                let same = st.items().len() == items.len()
                    && st
                        .items()
                        .iter()
                        .zip(&items)
                        .all(|(a, b)| a.label == b.label);
                if !same {
                    st.set_items(items.clone(), cx);
                }
                st.set_selected_value(&selected, cx);
            });
            let pick = pick.clone();
            TlSelect::new(ElementId::Name(id.clone()), &state).on_change(move |picked, _, cx| {
                if let Ok(i) = picked.parse::<usize>() {
                    pick(i, cx);
                }
            })
        });
        self.push(group, key, label, None, restart, field)
    }

    /// A slider over `range` in steps of `step`, its value text from `fmt`. Change writes in
    /// memory (`TlSettings::preview`); Release persists (`TlSettings::update`).
    pub fn slider(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        range: RangeInclusive<i64>,
        step: i64,
        fmt: fn(i64) -> SharedString,
    ) -> &mut Self {
        let restart = self.take_restart();
        let disabled = self.disable_next;
        self.sliders.push(SliderSpec {
            key,
            range: range.clone(),
            step,
            on_commit: self.commit_next.take(),
        });
        let (min, max) = (*range.start() as f32, *range.end() as f32);
        let field = SettingField::render(move |_, _, cx| {
            let value = read(cx, key).as_i64().unwrap_or(*range.start());
            match cx.try_global::<SliderStates>().and_then(|s| s.get(key)) {
                Some(state) => TlSlider::new(state)
                    .range(min, max, step as f32)
                    .value_text(fmt(value))
                    .disabled(disabled)
                    .into_any_element(),
                None => div().into_any_element(),
            }
        });
        self.push(group, key, label, None, restart, field)
    }

    /// A number input over `range` in steps of `step`; `suffix` (the unit) is the item's
    /// description.
    pub fn number(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        range: RangeInclusive<i64>,
        step: i64,
        suffix: SharedString,
    ) -> &mut Self {
        let restart = self.take_restart();
        let options = NumberFieldOptions {
            min: *range.start() as f64,
            max: *range.end() as f64,
            step: step as f64,
        };
        let toast = self.take_toast(&restart);
        let (min, max) = (*range.start(), *range.end());
        let setter: NumberSetter = Rc::new(move |v: f64, cx| {
            TlSettings::set(cx, key, SettingValue::Int(snap_number(v, min, max, step)));
            if let Some(note) = &toast {
                info_toast(note.clone(), cx);
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.numbers.push((key, setter.clone()));
        let field = SettingField::number_input(
            options,
            move |cx| read(cx, key).as_f64().unwrap_or_default(),
            move |v: f64, cx| setter(v, cx),
        );
        let description = (!suffix.is_empty()).then_some(suffix);
        self.push(group, key, label, description, restart, field)
    }

    /// A custom control (device list, fans) that binds `keys` itself.
    pub fn custom(
        &mut self,
        group: impl Into<SharedString>,
        keys: &[SettingKey],
        label: impl Into<SharedString>,
        render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> &mut Self {
        self.keys.extend_from_slice(keys);
        let render: CustomRender = Rc::new(render);
        let label: SharedString = label.into();
        #[cfg(any(test, feature = "test-hooks"))]
        self.customs.push((label.clone(), render.clone()));
        let item = SettingItem::new(
            label,
            SettingField::render(move |_, window, cx| render(window, cx)),
        );
        self.push_item(group, item);
        self
    }

    /// A button that dispatches `action`. Binds no key.
    pub fn button(
        &mut self,
        group: impl Into<SharedString>,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        style: TlButtonKind,
    ) -> &mut Self {
        let label: SharedString = label.into();
        let text = label.clone();
        let action: Rc<dyn Action> = Rc::from(action);
        #[cfg(any(test, feature = "test-hooks"))]
        self.buttons.push((label.clone(), action.clone()));
        let item = SettingItem::new(
            label,
            SettingField::render(move |_, _, _| {
                let action = action.clone();
                TlButton::new(SharedString::from(format!("SettingsButton-{text}")), style)
                    .label(text.clone())
                    .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            }),
        );
        self.push_item(group, item);
        self
    }

    /// A button that runs `on_click`. Binds no key.
    pub fn button_fn(
        &mut self,
        group: impl Into<SharedString>,
        label: impl Into<SharedString>,
        style: TlButtonKind,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> &mut Self {
        let label: SharedString = label.into();
        let text = label.clone();
        let on_click: ClickHandler = Rc::new(on_click);
        #[cfg(any(test, feature = "test-hooks"))]
        self.button_fns.push((label.clone(), on_click.clone()));
        let item = SettingItem::new(
            label,
            SettingField::render(move |_, _, _| {
                let on_click = on_click.clone();
                TlButton::new(SharedString::from(format!("SettingsButton-{text}")), style)
                    .label(text.clone())
                    .on_click(move |_, window, cx| on_click(window, cx))
            }),
        );
        self.push_item(group, item);
        self
    }

    /// A switch bound to no setting. Drawn as the kit row draws a switch, with the description
    /// under the title when `local.description` returns one.
    pub fn local_switch(
        &mut self,
        group: impl Into<SharedString>,
        label: impl Into<SharedString>,
        local: LocalSwitch,
    ) -> &mut Self {
        let label: SharedString = label.into();
        #[cfg(any(test, feature = "test-hooks"))]
        self.locals.push((label.clone(), local.clone()));
        let title = label.clone();
        let disabled = local.disabled;
        let item = SettingItem::render(move |options, _, cx| {
            let muted = current(cx).text_secondary;
            let set = local.set.clone();
            let mut text = v_flex()
                .flex_1()
                .max_w_3_5()
                .child(Label::new(title.clone()).text_sm());
            if let Some(description) = (local.description)() {
                text = text.child(div().text_sm().text_color(muted).child(description));
            }
            let switch = Switch::new("check")
                .checked((local.get)())
                .disabled(options.is_disabled())
                .with_size(options.size())
                .on_click(move |on: &bool, _, cx: &mut App| {
                    set(*on, cx);
                    cx.refresh_windows();
                });
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_3()
                .child(text)
                .child(div().id("field").child(switch))
        })
        .keywords([label])
        .disabled(disabled);
        self.push_item(group, item);
        self
    }

    /// The kit page with the page's groups, titled and with its icon.
    pub fn into_page(self, cx: &App) -> SettingPage {
        let title = tr(cx, self.id.title_key());
        let icon = self.id.icon();
        let description = self.description.clone();
        let page = SettingPage::new(title)
            .icon(icon)
            .groups(self.kit_groups(cx));
        match description {
            Some(text) => page.description(text),
            None => page,
        }
    }

    /// The kit groups of the page.
    ///
    /// The kit sidebar draws a chevron and one sub-entry per titled group on any page with two
    /// or more groups. A page with fewer than [`MIN_SIDEBAR_GROUPS`] groups is not worth that,
    /// so its groups become one kit group and each title a heading row in it. Every group keeps
    /// [`SCROLLBAR_GAP`] free at its right, since the kit draws its scrollbar over the right
    /// edge of the page.
    pub fn kit_groups(self, cx: &App) -> Vec<SettingGroup> {
        let s = metrics(cx).ui_scale;
        let groups: Vec<(SharedString, Vec<SettingItem>)> =
            if self.groups.len() < MIN_SIDEBAR_GROUPS {
                let theme = current(cx).clone();
                let m = *metrics(cx);
                let mut items = Vec::new();
                for (title, group_items) in self.groups {
                    if !title.is_empty() {
                        let theme = theme.clone();
                        items.push(SettingItem::render(move |_, _, _| {
                            let mut heading = div()
                                .text_color(theme.text_secondary)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title.clone());
                            *heading.text_style() = t_sm(&m);
                            heading
                        }));
                    }
                    items.extend(group_items);
                }
                vec![(SharedString::default(), items)]
            } else {
                self.groups
            };
        groups
            .into_iter()
            .map(|(title, items)| {
                let group = SettingGroup::new().items(items).pr(px(SCROLLBAR_GAP * s));
                if title.is_empty() {
                    group
                } else {
                    group.title(title)
                }
            })
            .collect()
    }

    /// True when the kit search for `query` finds an item on this page: the same rule as the kit
    /// (title, description or keyword contains the query, case folded; a custom row only by
    /// keyword). An empty query matches everything.
    pub fn matches_search(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.groups
            .iter()
            .flat_map(|(_, items)| items)
            .any(|item| match item {
                SettingItem::Item {
                    title,
                    description,
                    keywords,
                    ..
                } => {
                    title.to_lowercase().contains(&q)
                        || matches!(description, Some(Text::String(d)) if d.to_lowercase().contains(&q))
                        || keywords.iter().any(|k| k.to_lowercase().contains(&q))
                }
                SettingItem::Element { keywords, .. } => {
                    q.is_empty() || keywords.iter().any(|k| k.to_lowercase().contains(&q))
                }
            })
    }

    /// Runs the switch or checkbox setter of `key` as a click on it would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_bool(&self, key: SettingKey, on: bool, cx: &mut App) {
        if let Some((_, setter)) = self.bool_setters.iter().find(|(k, _)| *k == key) {
            setter(on, cx);
        }
    }

    /// Runs the number input setter of `key` as typing `v` would. It snaps to the nearest step
    /// from the minimum and clamps to the range.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_number(&self, key: SettingKey, v: f64, cx: &mut App) {
        if let Some((_, setter)) = self.numbers.iter().find(|(k, _)| *k == key) {
            setter(v, cx);
        }
    }

    /// The option labels of the dropdown of `key`, in order. Empty when there is none.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn dropdown_labels(&self, key: SettingKey) -> Vec<SharedString> {
        self.dropdowns
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, labels, _)| labels.clone())
            .unwrap_or_default()
    }

    /// Picks the option labelled `label` in the dropdown of `key`, as a click on it would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn pick(&self, key: SettingKey, label: &str, cx: &mut App) {
        if let Some((_, labels, setter)) = self.dropdowns.iter().find(|(k, _, _)| *k == key)
            && let Some(i) = labels.iter().position(|l| l.as_ref() == label)
        {
            setter(i, cx);
        }
    }

    /// Runs the checklist setter of `value` in `key`, as a click on its checkbox would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_check(&self, key: SettingKey, value: &str, on: bool, cx: &mut App) {
        if let Some((_, _, set)) = self
            .checks
            .iter()
            .find(|(k, v, _)| *k == key && v.as_ref() == value)
        {
            set(on, cx);
        }
    }

    /// The checklist values of `key`, in order.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn check_values(&self, key: SettingKey) -> Vec<SharedString> {
        self.checks
            .iter()
            .filter(|(k, _, _)| *k == key)
            .map(|(_, v, _)| v.clone())
            .collect()
    }

    /// Dispatches the action of the button labelled `label`, as a click on it would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn press(&self, label: &str, window: &mut Window, cx: &mut App) {
        if let Some((_, on_click)) = self.button_fns.iter().find(|(l, _)| l.as_ref() == label) {
            on_click(window, cx);
        } else if let Some((_, action)) = self.buttons.iter().find(|(l, _)| l.as_ref() == label) {
            window.dispatch_action(action.boxed_clone(), cx);
        }
    }

    /// Whether the keyless switch labelled `label` is on. `false` when there is none.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn local_on(&self, label: &str) -> bool {
        self.local(label).is_some_and(|l| (l.get)())
    }

    /// The description the keyless switch labelled `label` shows now, if any.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn local_description(&self, label: &str) -> Option<SharedString> {
        self.local(label).and_then(|l| (l.description)())
    }

    /// True when the keyless switch labelled `label` is disabled.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn local_disabled(&self, label: &str) -> bool {
        self.local(label).is_some_and(|l| l.disabled)
    }

    /// Runs the setter of the keyless switch labelled `label`, as a click on it would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_local(&self, label: &str, on: bool, cx: &mut App) {
        if let Some(local) = self.local(label) {
            (local.set)(on, cx);
            cx.refresh_windows();
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    fn local(&self, label: &str) -> Option<&LocalSwitch> {
        self.locals
            .iter()
            .find(|(l, _)| l.as_ref() == label)
            .map(|(_, local)| local)
    }

    /// The element of the custom item labelled `label`, as the page renders it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_custom(
        &self,
        label: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.customs
            .iter()
            .find(|(l, _)| l.as_ref() == label)
            .map(|(_, render)| render(window, cx))
    }

    /// The text the checklist of `key` shows when it has no option.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn empty_text(&self, key: SettingKey) -> Option<SharedString> {
        self.empty_texts
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| t.clone())
    }

    /// True when the item of `key` is disabled.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn is_disabled(&self, key: SettingKey) -> bool {
        self.disabled.contains(&key)
    }

    /// The indent of the switch of `key`, in px before the UI scale; 0 when it has none.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn indent_of(&self, key: SettingKey) -> f32 {
        self.indents
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(0., |(_, indent)| *indent)
    }

    /// The tooltip of the switch of `key`, if any.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn tooltip_of(&self, key: SettingKey) -> Option<SharedString> {
        self.tooltips
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, text)| text.clone())
    }

    /// The banner texts, top first.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn banners(&self) -> &[SharedString] {
        &self.banners
    }

    /// The group titles, in page order. An untitled group has an empty title.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn group_titles(&self) -> Vec<SharedString> {
        self.groups.iter().map(|(title, _)| title.clone()).collect()
    }

    /// The page description, if any.
    pub fn page_description_text(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    /// The description the item of `key` shows, if any.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn description_of(&self, key: SettingKey) -> Option<SharedString> {
        self.descriptions
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, text)| text.clone())
    }

    /// Writes a Bool key and pushes the restart toast when the item needs one.
    fn bool_setter(&mut self, key: SettingKey, restart: Option<SharedString>) -> BoolSetter {
        let toast = self.take_toast(&restart);
        let setter: BoolSetter = Rc::new(move |on, cx| {
            TlSettings::set(cx, key, SettingValue::Bool(on));
            if let Some(note) = &toast {
                info_toast(note.clone(), cx);
            }
        });
        #[cfg(any(test, feature = "test-hooks"))]
        self.bool_setters.push((key, setter.clone()));
        setter
    }

    fn take_restart(&mut self) -> Option<SharedString> {
        self.restart_next.take()
    }

    /// The restart toast of an item with the restart note `restart`: the text from
    /// `restart_note_toast`, else the note itself.
    fn take_toast(&mut self, restart: &Option<SharedString>) -> Option<SharedString> {
        let own = self.restart_toast_next.take();
        restart
            .as_ref()
            .map(|note| own.unwrap_or_else(|| note.clone()))
    }

    /// A switch item. With an indent or a tooltip it is drawn as the kit row draws it (title
    /// left, switch right), since the kit item has neither.
    #[allow(clippy::too_many_arguments)]
    fn push_switch(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
        restart: Option<SharedString>,
        get: BoolGetter,
        set: BoolSetter,
    ) -> &mut Self {
        let indent = std::mem::take(&mut self.indent_next);
        let tooltip = self.tooltip_next.take();
        if indent == 0. && tooltip.is_none() {
            let field = SettingField::switch(move |cx| get(cx), move |on, cx| set(on, cx));
            return self.push(group, key, label, description, restart, field);
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            if indent != 0. {
                self.indents.push((key, indent));
            }
            if let Some(text) = &tooltip {
                self.tooltips.push((key, text.clone()));
            }
        }
        self.keys.push(key);
        self.restart_toast_next = None;
        let label: SharedString = label.into();
        let description = restart.or(description);
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(text) = &description {
            self.descriptions.push((key, text.clone()));
        }
        let disabled = std::mem::take(&mut self.disable_next);
        #[cfg(any(test, feature = "test-hooks"))]
        if disabled {
            self.disabled.push(key);
        }
        let title = label.clone();
        let item = SettingItem::render(move |options, _, cx| {
            let s = metrics(cx).ui_scale;
            let muted = current(cx).text_secondary;
            let set = set.clone();
            let mut text = v_flex()
                .flex_1()
                .max_w_3_5()
                .child(Label::new(title.clone()).text_sm());
            if let Some(description) = &description {
                text = text.child(div().text_sm().text_color(muted).child(description.clone()));
            }
            let mut switch = Switch::new("check")
                .checked(get(cx))
                .disabled(options.is_disabled())
                .with_size(options.size())
                .on_click(move |on: &bool, _, cx: &mut App| set(*on, cx));
            if let Some(tip) = &tooltip {
                switch = switch.tooltip(tip.clone());
            }
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_3()
                .pl(px(indent * s))
                .child(text)
                .child(div().id("field").child(switch))
        })
        .keywords([label])
        .disabled(disabled);
        self.push_item(group, item);
        self
    }

    fn push<T: Clone + PartialEq + Send + Sync + 'static>(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
        restart: Option<SharedString>,
        field: SettingField<T>,
    ) -> &mut Self {
        self.keys.push(key);
        self.restart_toast_next = None;
        let mut item = SettingItem::new(label, field);
        if let Some(text) = restart.or(description) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.descriptions.push((key, text.clone()));
            item = item.description(text);
        }
        if std::mem::take(&mut self.disable_next) {
            #[cfg(any(test, feature = "test-hooks"))]
            self.disabled.push(key);
            item = item.disabled(true);
        }
        self.push_item(group, item);
        self
    }

    fn push_item(&mut self, group: impl Into<SharedString>, item: SettingItem) {
        let group = group.into();
        match self.groups.iter_mut().find(|(title, _)| *title == group) {
            Some((_, items)) => items.push(item),
            None => self.groups.push((group, vec![item])),
        }
    }

    /// Puts `item` back at the end of the last group.
    fn push_item_raw(&mut self, item: SettingItem) {
        if let Some((_, items)) = self.groups.last_mut() {
            items.push(item);
        }
    }
}

/// The current value of `key`, read from the settings as their serialized form. `Null` when
/// there are no settings or the key is not found.
fn read(cx: &App, key: SettingKey) -> serde_json::Value {
    if !cx.has_global::<TlSettings>() {
        return serde_json::Value::Null;
    }
    let Ok(all) = serde_json::to_value(TlSettings::settings(cx)) else {
        return serde_json::Value::Null;
    };
    let pointer = format!("/{}", key.as_str().replace('.', "/"));
    all.pointer(&pointer)
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// The members of the StrList `key`.
fn read_list(cx: &App, key: SettingKey) -> Vec<String> {
    match read(cx, key) {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Adds `value` to the StrList `key` when `present`, else removes it. Writes only on a change.
fn set_member(cx: &mut App, key: SettingKey, value: &str, present: bool) {
    let mut members = read_list(cx, key);
    let has = members.iter().any(|v| v == value);
    if has == present {
        return;
    }
    if present {
        members.push(value.to_owned());
    } else {
        members.retain(|v| v != value);
    }
    let list: Arc<[SharedString]> = members.into_iter().map(SharedString::from).collect();
    TlSettings::set(cx, key, SettingValue::StrList(list));
}

/// Like [`set_member`], but an added value goes to its place in `order` (values not in `order`
/// stay at the end), and with `keep_one` a removal that would leave no value of `order` is
/// refused with `keep_one` as an Info toast.
fn set_member_ordered(
    cx: &mut App,
    key: SettingKey,
    value: &str,
    present: bool,
    order: &[SharedString],
    keep_one: Option<&SharedString>,
) {
    let mut members = read_list(cx, key);
    let has = members.iter().any(|v| v == value);
    if has == present {
        return;
    }
    if present {
        members.push(value.to_owned());
        members.sort_by_key(|v| {
            order
                .iter()
                .position(|o| o.as_ref() == v)
                .unwrap_or(usize::MAX)
        });
    } else {
        members.retain(|v| v != value);
        if let Some(text) = keep_one
            && !members
                .iter()
                .any(|v| order.iter().any(|o| o.as_ref() == v))
        {
            info_toast(text.clone(), cx);
            return;
        }
    }
    let list: Arc<[SharedString]> = members.into_iter().map(SharedString::from).collect();
    TlSettings::set(cx, key, SettingValue::StrList(list));
}

fn read_bool(cx: &App, key: SettingKey) -> bool {
    read(cx, key).as_bool().unwrap_or_default()
}

/// True when the stored setting `now` equals the option value `v`.
fn value_matches(v: &SettingValue, now: &serde_json::Value) -> bool {
    match v {
        SettingValue::Bool(b) => now.as_bool() == Some(*b),
        SettingValue::Int(i) => now.as_i64() == Some(*i),
        SettingValue::Str(s) => now.as_str() == Some(s.as_ref()),
        SettingValue::StrList(list) => now.as_array().is_some_and(|a| {
            a.len() == list.len()
                && a.iter()
                    .zip(list.iter())
                    .all(|(x, y)| x.as_str() == Some(y))
        }),
    }
}

/// Dispatches `action` once the current event is done: a setter runs while the window handles
/// the click, so the window is busy and a dispatch now would be lost.
fn dispatch_deferred(action: Box<dyn Action>, cx: &mut App) {
    cx.defer(move |cx| cx.dispatch_action(action.as_ref()));
}

/// Pushes `text` (such as "Takes effect after restart") as an Info toast in the active window.
/// Deferred: a setter runs while the window handles the click, so the window is busy.
fn info_toast(text: SharedString, cx: &mut App) {
    kind_toast(ToastKind::Info, text, cx);
}

/// Pushes `text` as a toast of `kind` in the active window, deferred like [`info_toast`].
fn kind_toast(kind: ToastKind, text: SharedString, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let pushed = window.update(cx, |_, window, cx| {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
            }
        });
        if pushed.is_err() {
            warn!(target: "tasklens::ui", "settings toast: the window is gone");
        }
    });
}

/// The Settings window content.
pub struct SettingsWindow {
    sliders: Vec<(SettingKey, Entity<SliderState>)>,
    /// The kit's search field. The kit keeps it private; it is the first `InputState` its
    /// `Settings` creates after this window is built, so an observer on new `InputState`s picks it
    /// up (the observer gets no window for entities made inside a render).
    search: Rc<RefCell<Option<Entity<InputState>>>>,
    /// Armed by the first render, dropped once the field is found, so inputs created elsewhere
    /// later (or before the first render) are never taken for the search field.
    _search_capture: Option<Subscription>,
    _search_sub: Option<Subscription>,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u64,
    _slider_subs: Vec<Subscription>,
    _settings_sub: Subscription,
    _hub_sub: Option<Subscription>,
}

impl SettingsWindow {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search: Rc<RefCell<Option<Entity<InputState>>>> = Rc::default();
        let specs: Vec<SliderSpec> = all_pages(cx)
            .into_iter()
            .flat_map(|page| page.sliders)
            .collect();
        let mut sliders = Vec::with_capacity(specs.len());
        let mut slider_subs = Vec::with_capacity(specs.len());
        for spec in specs {
            let value = read(cx, spec.key).as_i64().unwrap_or(*spec.range.start());
            let state = cx.new(|_| {
                SliderState::new()
                    .min(*spec.range.start() as f32)
                    .max(*spec.range.end() as f32)
                    .step(spec.step as f32)
                    .default_value(value as f32)
            });
            let key = spec.key;
            let on_commit = spec.on_commit;
            slider_subs.push(
                cx.subscribe(&state, move |_, _, ev: &TlSliderEvent, cx| match *ev {
                    TlSliderEvent::Preview(v) => {
                        let value = SettingValue::Int(v.round() as i64);
                        TlSettings::preview(cx, |d| {
                            set_in(&mut d.settings, key, value);
                        });
                    }
                    TlSliderEvent::Commit(v) => {
                        let value = v.round() as i64;
                        TlSettings::set(cx, key, SettingValue::Int(value));
                        if let Some(on_commit) = on_commit {
                            on_commit(value, cx);
                        }
                    }
                }),
            );
            sliders.push((key, state));
        }
        cx.set_global(SliderStates(sliders.clone()));
        cx.on_release(|_, cx| {
            cx.remove_global::<SliderStates>();
        })
        .detach();

        let settings_sub = cx.observe_global::<TlSettings>(|_, cx| cx.notify());
        let hub_sub = cx
            .try_global::<GlobalHub>()
            .map(|h| h.0.clone())
            .map(|hub| {
                cx.subscribe(&hub, |_, _, ev: &SnapshotChanged, cx| {
                    if ev
                        .mask
                        .intersects(ComponentMask::DEVICES | ComponentMask::HEALTH)
                    {
                        cx.notify();
                    }
                })
            });
        Self {
            sliders,
            search,
            _search_capture: None,
            _search_sub: None,
            #[cfg(any(test, feature = "test-hooks"))]
            renders: 0,
            _slider_subs: slider_subs,
            _settings_sub: settings_sub,
            _hub_sub: hub_sub,
        }
    }

    /// The slider state of `key`, if a page has a slider for it.
    pub fn slider_state(&self, key: SettingKey) -> Option<&Entity<SliderState>> {
        self.sliders.iter().find(|(k, _)| *k == key).map(|(_, s)| s)
    }

    /// How often the window rendered.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn render_count(&self) -> u64 {
        self.renders
    }
}

/// Applies `value` to `key` in `settings` through the same path as `TlSettings::set`, for the
/// in-memory slider preview.
fn set_in(settings: &mut tl_model::settings::Settings, key: SettingKey, value: SettingValue) {
    let mut probe = serde_json::to_value(&*settings).unwrap_or(serde_json::Value::Null);
    let pointer = format!("/{}", key.as_str().replace('.', "/"));
    let new = match value {
        SettingValue::Int(i) => serde_json::Value::from(i),
        SettingValue::Bool(b) => serde_json::Value::from(b),
        SettingValue::Str(s) => serde_json::Value::from(s.to_string()),
        SettingValue::StrList(l) => {
            serde_json::Value::from(l.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        }
    };
    if let Some(slot) = probe.pointer_mut(&pointer) {
        *slot = new;
        if let Ok(next) = serde_json::from_value(probe) {
            *settings = next;
        }
    }
}

impl SettingsWindow {
    /// Subscribes to the kit's search field once it exists, so typing re-renders this window.
    fn attach_search(&mut self, cx: &mut Context<Self>) {
        if self._search_sub.is_some() {
            return;
        }
        let Some(input) = self.search.borrow().clone() else {
            return;
        };
        self._search_capture = None;
        self._search_sub = Some(cx.subscribe(&input, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));
        cx.notify();
    }

    /// The text in the search field.
    fn query(&self, cx: &App) -> SharedString {
        self.search
            .borrow()
            .as_ref()
            .map(|input| input.read(cx).value())
            .unwrap_or_default()
    }

    /// Sets the search text as typing it would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.search.borrow().clone().expect("search field exists");
        input.update(cx, |state, cx| state.set_value(text.to_owned(), window, cx));
        cx.notify();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        if self._search_sub.is_none() {
            if self._search_capture.is_none() {
                let slot = self.search.clone();
                self._search_capture = Some(cx.observe_new(
                    move |_: &mut InputState,
                          _: Option<&mut Window>,
                          cx: &mut Context<InputState>| {
                        let mut slot = slot.borrow_mut();
                        if slot.is_none() {
                            *slot = Some(cx.entity());
                        }
                    },
                ));
            }
            cx.defer_in(window, |this, _, cx| this.attach_search(cx));
        }
        let s = metrics(cx).ui_scale;
        let builders: Vec<PageBuilder> = all_pages(cx)
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect();
        let query = self.query(cx);
        let no_match = !query.is_empty() && !builders.iter().any(|p| p.matches_search(&query));
        let pages: Vec<SettingPage> = builders.into_iter().map(|p| p.into_page(cx)).collect();
        let theme = current(cx).clone();
        let m = *metrics(cx);
        div()
            .debug_selector(|| "SettingsWindowBody".into())
            .size_full()
            .relative()
            .child(
                KitSettings::new("settings")
                    .sidebar_width(px(SIDEBAR_W * s))
                    .pages(pages),
            )
            .when(no_match, |d| {
                // The kit draws a blank page when nothing matches. Cover the page area (right of
                // the sidebar) with the message.
                let mut text = div()
                    .debug_selector(|| "SettingsNoMatch".into())
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .left(px(SIDEBAR_W * s))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_secondary)
                    .child(tr(cx, "shell-settings-no-match"));
                *text.text_style() = t_sm(&m);
                d.child(text)
            })
    }
}
