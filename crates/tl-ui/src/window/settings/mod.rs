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
//! (`ComponentMask::DEVICES`, for the Performance page lists), never on other snapshots. Slider
//! states are created in `SettingsWindow::new`, never in render.

use std::ops::RangeInclusive;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::slider::SliderState;
use gpui_kit::component::setting::{
    NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage,
    Settings as KitSettings,
};
use gpui_kit::{
    Action, AnyElement, App, AppContext as _, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px, size,
};
use tracing::warn;

use super::main::ShellState;
use super::registry::{GlobalRegistry, WindowKey};
use crate::actions::OpenSettings;
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::slider::{TlSlider, TlSliderEvent};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged};
use crate::i18n::tr;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::theme::metrics;

/// Window size and minimum size before the UI scale.
pub const SIZE: (f32, f32) = (900., 640.);
pub const MIN_SIZE: (f32, f32) = (720., 480.);
/// Sidebar width before the UI scale.
pub const SIDEBAR_W: f32 = 220.;

const TITLE: &str = "shell-settings-title";
const RESTART_NOTE: &str = "shell-settings-restart-note";

/// Registers the `OpenSettings` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
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
    #[allow(unused_mut, reason = "the page beads push their pages here")]
    let mut pages: Vec<PageBuilder> = Vec::new();
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(test) = cx.try_global::<TestPages>().map(|t| t.0) {
        pages.extend(test(cx));
    }
    let _ = cx;
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

type CustomRender = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// Builds one page: groups of items, each control bound to one setting. `keys` records every
/// key a control binds, in order.
pub struct PageBuilder {
    id: PageId,
    groups: Vec<(SharedString, Vec<SettingItem>)>,
    keys: Vec<SettingKey>,
    sliders: Vec<SliderSpec>,
    /// "Takes effect after restart": the next item gets it as description and as a toast.
    restart_next: Option<SharedString>,
}

impl PageBuilder {
    pub fn new(id: PageId) -> Self {
        Self {
            id,
            groups: Vec::new(),
            keys: Vec::new(),
            sliders: Vec::new(),
            restart_next: None,
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
        let field = SettingField::switch(
            move |cx| read_bool(cx, key),
            bool_setter(key, restart.clone()),
        );
        self.push(group, key, label, description, restart, field)
    }

    pub fn checkbox(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
    ) -> &mut Self {
        let restart = self.take_restart();
        let field = SettingField::checkbox(
            move |cx| read_bool(cx, key),
            bool_setter(key, restart.clone()),
        );
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
        let values: Rc<[SettingValue]> = options.iter().map(|(_, v)| v.clone()).collect();
        let kit_options = options
            .iter()
            .enumerate()
            .map(|(i, (label, _))| (SharedString::from(i.to_string()), label.clone()))
            .collect();
        let read_values = values.clone();
        let toast = restart.clone();
        let field = SettingField::dropdown(
            kit_options,
            move |cx| {
                let now = read(cx, key);
                read_values
                    .iter()
                    .position(|v| value_matches(v, &now))
                    .map(|i| SharedString::from(i.to_string()))
                    .unwrap_or_default()
            },
            move |picked: SharedString, cx| {
                let Some(value) = picked.parse::<usize>().ok().and_then(|i| values.get(i)) else {
                    return;
                };
                TlSettings::set(cx, key, value.clone());
                if let Some(note) = &toast {
                    restart_toast(note.clone(), cx);
                }
            },
        );
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
        self.sliders.push(SliderSpec {
            key,
            range: range.clone(),
            step,
        });
        let (min, max) = (*range.start() as f32, *range.end() as f32);
        let field = SettingField::render(move |_, _, cx| {
            let value = read(cx, key).as_i64().unwrap_or(*range.start());
            match cx.try_global::<SliderStates>().and_then(|s| s.get(key)) {
                Some(state) => TlSlider::new(state)
                    .range(min, max, step as f32)
                    .value_text(fmt(value))
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
        let toast = restart.clone();
        let field = SettingField::number_input(
            options,
            move |cx| read(cx, key).as_f64().unwrap_or_default(),
            move |v: f64, cx| {
                TlSettings::set(cx, key, SettingValue::Int(v.round() as i64));
                if let Some(note) = &toast {
                    restart_toast(note.clone(), cx);
                }
            },
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

    /// The kit page with the page's groups, titled and with its icon.
    pub fn into_page(self, cx: &App) -> SettingPage {
        let groups = self.groups.into_iter().map(|(title, items)| {
            let group = SettingGroup::new().items(items);
            if title.is_empty() {
                group
            } else {
                group.title(title)
            }
        });
        SettingPage::new(tr(cx, self.id.title_key()))
            .icon(self.id.icon())
            .groups(groups)
    }

    fn take_restart(&mut self) -> Option<SharedString> {
        self.restart_next.take()
    }

    fn push<T: 'static>(
        &mut self,
        group: impl Into<SharedString>,
        key: SettingKey,
        label: impl Into<SharedString>,
        description: Option<SharedString>,
        restart: Option<SharedString>,
        field: SettingField<T>,
    ) -> &mut Self {
        self.keys.push(key);
        let mut item = SettingItem::new(label, field);
        if let Some(description) = description {
            item = item.description(description);
        }
        if let Some(note) = restart {
            item = item.description(note);
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

fn bool_setter(key: SettingKey, restart: Option<SharedString>) -> impl Fn(bool, &mut App) {
    move |on, cx| {
        TlSettings::set(cx, key, SettingValue::Bool(on));
        if let Some(note) = &restart {
            restart_toast(note.clone(), cx);
        }
    }
}

/// Pushes `text` ("Takes effect after restart") as an Info toast in the active window.
fn restart_toast(text: SharedString, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let pushed = window.update(cx, |_, window, cx| {
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
        }
    });
    if pushed.is_err() {
        warn!(target: "tasklens::ui", "restart toast: the window is gone");
    }
}

/// The Settings window content.
pub struct SettingsWindow {
    sliders: Vec<(SettingKey, Entity<SliderState>)>,
    #[cfg(any(test, feature = "test-hooks"))]
    renders: u64,
    _slider_subs: Vec<Subscription>,
    _settings_sub: Subscription,
    _hub_sub: Option<Subscription>,
}

impl SettingsWindow {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            slider_subs.push(
                cx.subscribe(&state, move |_, _, ev: &TlSliderEvent, cx| match *ev {
                    TlSliderEvent::Preview(v) => {
                        let value = SettingValue::Int(v.round() as i64);
                        TlSettings::preview(cx, |d| {
                            set_in(&mut d.settings, key, value);
                        });
                    }
                    TlSliderEvent::Commit(v) => {
                        TlSettings::set(cx, key, SettingValue::Int(v.round() as i64));
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
                    if ev.mask.intersects(ComponentMask::DEVICES) {
                        cx.notify();
                    }
                })
            });
        Self {
            sliders,
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

impl Render for SettingsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(test, feature = "test-hooks"))]
        {
            self.renders += 1;
        }
        let s = metrics(cx).ui_scale;
        let pages: Vec<SettingPage> = all_pages(cx)
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(|p| p.into_page(cx))
            .collect();
        div()
            .debug_selector(|| "SettingsWindowBody".into())
            .size_full()
            .child(
                KitSettings::new("settings")
                    .sidebar_width(px(SIDEBAR_W * s))
                    .pages(pages),
            )
    }
}
