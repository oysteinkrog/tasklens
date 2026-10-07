//! The Process Properties window: open, focus-existing and cap rules, the tab strip with counts,
//! the data path, keys, the exited state and the access block (UI-PWN-002, UI-PWN-003,
//! UI-PWN-004, UI-PWN-018, PROC-077, PROC-071, R-119, ADR-003, ADR-008, D-008).
//!
//! The window registers `DetailDemand { Process(key), active tab }` while it is open and reads
//! `Snapshot.processes`, `Snapshot.system` and `Snapshot.details`. A snapshot costs one record
//! lookup and one `Arc` pointer compare per payload it shows. The window itself re-renders only
//! when a tab label, the active tab or the exited state changes; the tab views are fed through
//! their own setters and notify themselves.
//!
//! Tab views other than General come from later beads (E23-016 to E23-021). Until a tab has its
//! view, its body is empty, or shows the access block when its payload is `Restricted`.

pub mod filter;
pub mod general;

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable as _, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StyleRefinement, Styled as _, Subscription, Window, WindowHandle,
    div, prelude::FluentBuilder as _, px,
};
use tl_model::command::Command;
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{DetailTab, DetailTarget, TabSlot, TargetDetail};
use tl_model::ids::ProcessKey;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use self::filter::{PropsFilter, filter_bar, on_clear};
use self::general::GeneralTab;
use crate::app::EngineGlobal;
use crate::components::menu::{close_context_menu, context_menu_open};
use crate::components::tabs::{TlTabSpec, TlTabs};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::TlSecondaryWindow;
use crate::demand::GlobalDemand;
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::Strings;
use crate::pages::performance::details::Template;
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};
use crate::windows::actions::{
    PROPS_WINDOW_CONTEXT, PropsCloseWindow, PropsEscape, PropsFocusFilter, PropsNextTab,
    PropsPrevTab, PropsRefreshNow, PropsSelectTab, PropsVerifyPackage,
};
use crate::windows::frame::{EscStep, PwnWindow, esc_step, open_pwn, props_title};

/// Access block texts (UI-PWN-018, P2). [`access_block`] shows the translated text.
pub const ACCESS_FDS: &str = "Handles unavailable (insufficient privileges)";
pub const ACCESS_MAPS: &str = "Needs administrator access.";
pub const ACCESS_SECURITY: &str = "Security information unavailable (insufficient privileges)";
pub const ACCESS_ENV: &str = "Environment unavailable (insufficient privileges)";
pub const ACCESS_STRINGS: &str = "Strings unavailable: needs administrator rights";

/// The tabs of the window in strip order. `PropsSelectTab(n)` selects `TABS[n - 1]`.
pub const TABS: [DetailTab; 7] = [
    DetailTab::General,
    DetailTab::Threads,
    DetailTab::Fds,
    DetailTab::Maps,
    DetailTab::Security,
    DetailTab::Environment,
    DetailTab::Strings,
];

/// Banner height, × ui_scale.
const BANNER_H: f32 = 24.;
/// Banner icon size, × ui_scale.
const BANNER_ICON: f32 = 14.;
/// Banner fill and border alpha of `status.warning`.
const BANNER_FILL_ALPHA: f32 = 0.15;
const BANNER_BORDER_ALPHA: f32 = 0.40;
/// Tab strip height, × ui_scale (UI-FND-018).
const STRIP_H: f32 = 32.;
/// Frame title bar height, × ui_scale (UI-FND-021).
const TITLE_BAR_H: f32 = 34.;
/// Gap between the tab strip and the toast stack, × ui_scale.
const TOAST_GAP: f32 = 8.;
/// Access block: distance from the body top, icon size and text width, × ui_scale.
const ACCESS_TOP: f32 = 64.;
const ACCESS_ICON: f32 = 24.;
const ACCESS_MAX_W: f32 = 420.;

const KEY_TABS: [&str; 7] = [
    "pwn-props-tab-general",
    "pwn-props-tab-threads",
    "pwn-props-tab-fds",
    "pwn-props-tab-maps",
    "pwn-props-tab-security",
    "pwn-props-tab-environment",
    "pwn-props-tab-strings",
];
const KEY_TAB_TIP: &str = "pwn-props-tab-tooltip";
const KEY_TOO_MANY: &str = "pwn-props-too-many-windows";
const KEY_OPEN_FAILED: &str = "fnd-props-open-failed";
const KEY_EXITED: &str = "pwn-props-exited-banner";
const KEY_NOT_RUNNING: &str = "pwn-props-not-running";
const KEY_ACCESS: [(&str, &str); 5] = [
    (ACCESS_FDS, "pwn-props-access-fds"),
    (ACCESS_MAPS, "pwn-props-access-maps"),
    (ACCESS_SECURITY, "pwn-props-access-security"),
    (ACCESS_ENV, "pwn-props-access-env"),
    (ACCESS_STRINGS, "pwn-props-access-strings"),
];

// ---- Words (R-132) ----

/// The window texts, translated once per strings version by [`props_words`].
pub struct PropsWords {
    /// Tab labels in strip order, without counts.
    pub tabs: [SharedString; 7],
    /// Tab tooltips in strip order, such as "Threads (Alt+2)".
    pub tab_tips: [SharedString; 7],
    pub too_many: SharedString,
    pub open_failed: SharedString,
    pub exited_banner: SharedString,
    /// Tooltip of every action control while the process is not running.
    pub not_running: SharedString,
    /// The access block texts, in [`KEY_ACCESS`] order.
    access: [SharedString; 5],
}

impl PropsWords {
    fn with(t: impl Fn(&str) -> SharedString, tab_tip: Template) -> Self {
        let tabs = KEY_TABS.map(&t);
        let tab_tips = std::array::from_fn(|i| {
            let mut tip = String::new();
            let _ = tab_tip.write(&mut tip, &[&tabs[i], &(i + 1)]);
            tip.into()
        });
        Self {
            tabs,
            tab_tips,
            too_many: t(KEY_TOO_MANY),
            open_failed: t(KEY_OPEN_FAILED),
            exited_banner: t(KEY_EXITED),
            not_running: t(KEY_NOT_RUNNING),
            access: KEY_ACCESS.map(|(_, key)| t(key)),
        }
    }

    fn en() -> Self {
        Self::with(
            |key| crate::i18n::tr_en(key, &[]),
            Template::en(KEY_TAB_TIP, &["label", "n"]),
        )
    }

    fn translated(cx: &App) -> Self {
        Self::with(
            |key| crate::i18n::tr(cx, key),
            Template::new(cx, KEY_TAB_TIP, &["label", "n"]),
        )
    }

    /// The translated text of an `ACCESS_*` constant; another text is shown as given.
    pub fn access(&self, text: &'static str) -> SharedString {
        KEY_ACCESS
            .iter()
            .position(|(en, _)| *en == text)
            .map_or_else(|| text.into(), |i| self.access[i].clone())
    }
}

struct PropsWordsCache {
    version: u64,
    words: Rc<PropsWords>,
}

impl Global for PropsWordsCache {}

/// The texts for the current strings. Translated only when the strings version changed; without
/// strings they are in en-US.
pub fn props_words(cx: &mut App) -> Rc<PropsWords> {
    let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) else {
        return Rc::new(PropsWords::en());
    };
    if let Some(cache) = cx.try_global::<PropsWordsCache>()
        && cache.version == version
    {
        return cache.words.clone();
    }
    let words = Rc::new(PropsWords::translated(cx));
    cx.set_global(PropsWordsCache {
        version,
        words: words.clone(),
    });
    words
}

// ---- Access block (UI-PWN-018) ----

/// The block a tab shows instead of its body when its payload is `Restricted`.
#[derive(IntoElement)]
pub struct AccessBlock {
    text: &'static str,
}

/// Centred 64 px under the body top: the `shield-alert` icon, then `text` (an `ACCESS_*`
/// constant), translated. P2 has no button.
pub fn access_block(text: &'static str) -> impl IntoElement {
    AccessBlock { text }
}

impl RenderOnce for AccessBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let text = props_words(cx).access(self.text);
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let mut message = div()
            .debug_selector(|| "PropsAccessBlock-text".into())
            .max_w(px(ACCESS_MAX_W * s))
            .text_center()
            .text_color(theme.text_primary);
        *message.text_style() = t_base(&m);
        v_flex()
            .debug_selector(|| "PropsAccessBlock".into())
            .size_full()
            .items_center()
            .pt(px(ACCESS_TOP * s))
            .gap(m.sp[4])
            .child(
                // Decorative: the text says everything.
                Icon::new(IconName::ShieldAlert)
                    .size(px(ACCESS_ICON * s))
                    .text_color(theme.text_secondary),
            )
            .child(message.child(text))
    }
}

/// The access text of `tab`, for tabs that show the access block.
fn access_text(tab: DetailTab) -> Option<&'static str> {
    match tab {
        DetailTab::Fds => Some(ACCESS_FDS),
        DetailTab::Maps => Some(ACCESS_MAPS),
        DetailTab::Security => Some(ACCESS_SECURITY),
        DetailTab::Environment => Some(ACCESS_ENV),
        DetailTab::Strings => Some(ACCESS_STRINGS),
        _ => None,
    }
}

// ---- Opening ----

/// Opens the Properties window for `key`, or activates the one already open for it (its active
/// tab is kept). A ninth window is refused with a Warning toast in the source window; a key that
/// is missing or exited gets the Error toast "Could not open properties." in the main window.
pub fn open_process_properties(key: ProcessKey, cx: &mut App) {
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) else {
        warn!(target: "tasklens::ui", "no window registry, properties not opened");
        return;
    };
    let words = props_words(cx);
    let record = cx
        .try_global::<GlobalHub>()
        .and_then(|hub| hub.0.read(cx).current().processes.get(key).cloned())
        .filter(|r| !r.flags.contains(ProcFlags::EXITED));
    let source = cx.active_window();
    let mut opened = None;
    let outcome = registry.update(cx, |r, cx| {
        r.open_or_focus(WindowKey::Properties(key), cx, |cx| {
            let Some(record) = record else {
                return Err(gpui_kit::gpui::private::anyhow::anyhow!(
                    "process {} is not running",
                    key.pid
                ));
            };
            let title = props_title(&record.meta.name, key.pid, false);
            let handle = open_pwn(
                cx,
                PwnWindow::ProcessProperties,
                title,
                None,
                |window, cx| cx.new(|cx| PropsWindow::new(key, Some(record), window, cx)),
            )?;
            opened = Some(handle);
            Ok(handle.into())
        })
    });
    match outcome {
        OpenOutcome::Opened => {
            if let Some(handle) = opened {
                place_toasts(handle, cx);
            }
        }
        OpenOutcome::Focused => {}
        OpenOutcome::CapReached(_) => {
            let text = words.too_many.clone();
            let queue = source.and_then(|w| {
                w.update(cx, |_, window, cx| ToastQueue::of_window(window, cx))
                    .ok()
                    .flatten()
            });
            match queue {
                Some(queue) => {
                    queue.update(cx, |q, cx| q.push(ToastKind::Warning, text, None, None, cx))
                }
                None => push_main_toast(cx, ToastKind::Warning, text, None, None),
            }
        }
        OpenOutcome::Failed => {
            push_main_toast(cx, ToastKind::Error, words.open_failed.clone(), None, None);
        }
    }
}

/// Puts the window's toast stack 8 px under the tab strip.
fn place_toasts(handle: WindowHandle<Root>, cx: &mut App) {
    let Some(frame) = TlSecondaryWindow::<PropsWindow>::find(handle, cx) else {
        return;
    };
    let s = metrics(cx).ui_scale;
    let frame = frame.read(cx);
    let bar = if frame.has_title_bar() {
        TITLE_BAR_H
    } else {
        0.
    };
    let top = px((bar + STRIP_H + TOAST_GAP) * s);
    let toasts = frame.toasts().clone();
    toasts.update(cx, |q, cx| q.set_top(top, cx));
}

// ---- The window ----

/// The tab views, created when a tab is first shown. Later beads add their tab here.
#[derive(Default)]
pub struct PropsTabs {
    pub general: Option<Entity<GeneralTab>>,
}

/// The root view of a Process Properties window.
pub struct PropsWindow {
    key: ProcessKey,
    active: DetailTab,
    tabs: PropsTabs,
    filter: [Entity<PropsFilter>; 7],
    exited: bool,
    last_record: Option<ProcessRecord>,
    last_detail: Option<Arc<TargetDetail>>,
    /// Per tab: the count and the cached `[n]` suffix.
    labels: [(Option<u32>, SharedString); 7],
    name: SharedString,
    mem_total: u64,
    words: Rc<PropsWords>,
    focus: FocusHandle,
    /// The demand source id; `None` once the entry is removed.
    demand: Option<EntityId>,
    renders: u64,
    _subs: Vec<Subscription>,
}

impl PropsWindow {
    fn new(
        key: ProcessKey,
        record: Option<ProcessRecord>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = props_words(cx);
        let filter = std::array::from_fn(|i| {
            let label = words.tabs[i].clone();
            cx.new(|cx| PropsFilter::new(label, window, cx))
        });
        let mut subs = Vec::new();
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        if let Some(hub) = &hub {
            subs.push(cx.subscribe_in(
                hub,
                window,
                |this, hub, ev: &SnapshotChanged, window, cx| {
                    let snap = hub.read(cx).current().clone();
                    this.take_snapshot(&snap, ev.mask, window, cx);
                },
            ));
        }
        let name: SharedString = record
            .as_ref()
            .map_or_else(SharedString::default, |r| r.meta.name.to_string().into());
        let mut this = Self {
            key,
            active: DetailTab::General,
            tabs: PropsTabs::default(),
            filter,
            exited: false,
            last_record: None,
            last_detail: None,
            labels: Default::default(),
            name,
            mem_total: 0,
            words,
            focus: cx.focus_handle(),
            demand: None,
            renders: 0,
            _subs: subs,
        };
        this.ensure_tab(DetailTab::General, cx);

        // Demand (UI-PWN-002): removed on release.
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let id = cx.entity_id();
            this.demand = Some(id);
            this.register_demand(window, cx);
            cx.on_release(move |_, cx| demand.update(cx, |d, cx| d.remove(id, cx)))
                .detach();
        }
        this.send(Command::ComputeHashes(key), cx);

        if let Some(hub) = hub {
            let snap = hub.read(cx).current().clone();
            this.take_snapshot(&snap, ComponentMask::ALL, window, cx);
        }
        this.focus_first(window, cx);
        this
    }

    /// The content of a Properties window, while it is open.
    pub fn find(handle: AnyWindowHandle, cx: &App) -> Option<Entity<Self>> {
        let handle = WindowHandle::<Root>::new(handle.window_id());
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn active(&self) -> DetailTab {
        self.active
    }

    /// The process has exited: values are frozen and every action control is disabled.
    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn tabs(&self) -> &PropsTabs {
        &self.tabs
    }

    /// The filter of `tab`.
    pub fn filter(&self, tab: DetailTab) -> &Entity<PropsFilter> {
        &self.filter[strip_index(tab)]
    }

    pub fn last_record(&self) -> Option<&ProcessRecord> {
        self.last_record.as_ref()
    }

    pub fn last_detail(&self) -> Option<&Arc<TargetDetail>> {
        self.last_detail.as_ref()
    }

    /// The window texts.
    pub fn words(&self) -> &Rc<PropsWords> {
        &self.words
    }

    /// The label of `tab` with its count, such as "File descriptors [1 204]". For tests and the
    /// accessible name; the strip draws label and count as separate runs.
    pub fn tab_text(&self, tab: DetailTab) -> String {
        let i = strip_index(tab);
        match self.labels[i].0 {
            Some(_) => format!("{} {}", self.words.tabs[i], self.labels[i].1),
            None => self.words.tabs[i].to_string(),
        }
    }

    /// Times this view rendered, for the re-render tests.
    pub fn render_count(&self) -> u64 {
        self.renders
    }

    /// Sets the count shown after the label of `tab`. Tab beads report File descriptors, Memory
    /// maps and Environment counts here; Threads comes from the record. General, Security and
    /// Strings never show one.
    pub fn set_count(&mut self, tab: DetailTab, n: u32, cx: &mut Context<Self>) {
        if !matches!(
            tab,
            DetailTab::Threads | DetailTab::Fds | DetailTab::Maps | DetailTab::Environment
        ) {
            return;
        }
        let slot = &mut self.labels[strip_index(tab)];
        if slot.0 == Some(n) {
            return;
        }
        let mut text = String::from("[");
        units::count(&mut text, u64::from(n));
        text.push(']');
        *slot = (Some(n), text.into());
        cx.notify();
    }

    /// Makes `tab` active, updates the demand entry and focuses the tab.
    pub fn select_tab(&mut self, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.active {
            return;
        }
        self.active = tab;
        self.ensure_tab(tab, cx);
        self.register_demand(window, cx);
        self.focus_first(window, cx);
        cx.notify();
    }

    fn ensure_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if tab == DetailTab::General && self.tabs.general.is_none() {
            let filter = self.filter[0].clone();
            let key = self.key;
            self.tabs.general = Some(cx.new(|cx| GeneralTab::new(key, filter, cx)));
        }
    }

    fn register_demand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.demand.filter(|_| !self.exited) else {
            return;
        };
        let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) else {
            return;
        };
        let part = Demand {
            any_surface_visible: true,
            details: Arc::from([DetailDemand {
                target: DetailTarget::Process(self.key),
                tab: self.active,
            }]),
            ..Demand::default()
        };
        let handle = window.window_handle();
        demand.update(cx, |d, cx| d.set(id, Some(handle), part, cx));
    }

    fn remove_demand(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.demand.take() else {
            return;
        };
        if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            demand.update(cx, |d, cx| d.remove(id, cx));
        }
    }

    fn send(&self, cmd: Command, cx: &App) {
        let Some(engine) = cx.try_global::<EngineGlobal>() else {
            return;
        };
        if let Err(err) = engine.0.send(cmd) {
            warn!(target: "tasklens::ui", ?err, "properties command not sent");
        }
    }

    /// Focuses the active tab's first focusable element: the filter for General and Security,
    /// else the window root until the tab has its view.
    fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active {
            DetailTab::General | DetailTab::Security => self.focus_filter(window, cx),
            _ => self.focus.focus(window, cx),
        }
    }

    fn focus_filter(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.filter[strip_index(self.active)].read(cx).input.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Takes what this window shows from `snap`. `mask` is the set of changed components.
    fn take_snapshot(
        &mut self,
        snap: &Snapshot,
        mask: ComponentMask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let general = self.tabs.general.clone();
        if mask.intersects(ComponentMask::SYSTEM) {
            self.mem_total = snap.system.memory.total;
            let online = snap.system.cpu_topology.online;
            if let Some(g) = &general {
                g.update(cx, |g, cx| g.set_online_cpus(online, cx));
            }
        }
        if mask.intersects(ComponentMask::WINDOWS)
            && let Some(g) = &general
        {
            g.update(cx, |g, cx| g.set_windows(&snap.windows, cx));
        }
        if mask.intersects(ComponentMask::PROCESSES | ComponentMask::SYSTEM) {
            match snap.processes.get(self.key) {
                Some(record) => {
                    self.set_count(DetailTab::Threads, record.threads, cx);
                    let mem_total = self.mem_total;
                    if let Some(g) = &general {
                        g.update(cx, |g, cx| g.set_record(Some(record), mem_total, cx));
                    }
                    self.last_record = Some(record.clone());
                }
                // `processes` is the full table: a key missing from it has exited, whether or
                // not a snapshot showed it with `EXITED` before it was reaped.
                None if mask.intersects(ComponentMask::PROCESSES) => {
                    self.mark_exited(window, cx);
                    return;
                }
                None => {}
            }
        }
        if mask.intersects(ComponentMask::DETAILS) {
            let detail = snap
                .details
                .targets
                .get(&DetailTarget::Process(self.key))
                .cloned();
            let Some(detail) = detail else {
                return;
            };
            let missing = |slot: &TabSlot| slot.validity == Validity::Missing;
            if missing(detail.tab(DetailTab::General)) || missing(detail.tab(self.active)) {
                self.mark_exited(window, cx);
                return;
            }
            let shown = |d: &TargetDetail, tab| d.tab(tab).payload.clone();
            let changed = |tab| match &self.last_detail {
                Some(last) => !same_payload(&shown(last, tab), &shown(&detail, tab)),
                None => true,
            };
            let general_changed = changed(DetailTab::General);
            let active_changed = changed(self.active);
            let validity_changed = self.last_detail.as_ref().is_none_or(|last| {
                last.tab(self.active).validity != detail.tab(self.active).validity
            });
            if !(general_changed || active_changed || validity_changed) {
                return;
            }
            if general_changed && let Some(g) = &general {
                let payload = detail.tab(DetailTab::General).payload.clone();
                g.update(cx, |g, cx| g.set_payload(payload.as_ref(), cx));
            }
            self.last_detail = Some(detail);
            // A tab without its view shows the access block from the slot validity.
            if self.active != DetailTab::General && (active_changed || validity_changed) {
                cx.notify();
            }
        }
    }

    /// Freezes the window: the title gains " (not running)", the banner shows, the tab values
    /// stay as last shown, and the demand entry is removed. The window stays open.
    fn mark_exited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.exited = true;
        if let Some(g) = &self.tabs.general {
            g.update(cx, |g, cx| g.set_exited(true, cx));
        }
        self.remove_demand(cx);
        let title = props_title(&self.name, self.key.pid, true);
        let frame = window.root::<Root>().flatten().and_then(|root| {
            root.read(cx)
                .view()
                .clone()
                .downcast::<TlSecondaryWindow<Self>>()
                .ok()
        });
        match frame {
            Some(frame) => frame.update(cx, |f, cx| f.set_title(title, window, cx)),
            None => window.set_window_title(&title),
        }
        cx.notify();
    }

    fn on_select_tab(&mut self, a: &PropsSelectTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&tab) = usize::from(a.0).checked_sub(1).and_then(|i| TABS.get(i)) {
            self.select_tab(tab, window, cx);
        }
    }

    fn on_next_tab(&mut self, _: &PropsNextTab, window: &mut Window, cx: &mut Context<Self>) {
        let next = TABS[(strip_index(self.active) + 1) % TABS.len()];
        self.select_tab(next, window, cx);
    }

    fn on_prev_tab(&mut self, _: &PropsPrevTab, window: &mut Window, cx: &mut Context<Self>) {
        let n = TABS.len();
        let prev = TABS[(strip_index(self.active) + n - 1) % n];
        self.select_tab(prev, window, cx);
    }

    fn on_focus_filter(
        &mut self,
        _: &PropsFocusFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_filter(window, cx);
    }

    fn on_refresh(&mut self, _: &PropsRefreshNow, _: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        self.send(
            Command::DetailRefresh {
                target: DetailTarget::Process(self.key),
                tab: self.active,
            },
            cx,
        );
    }

    /// The Verify button leaves focus where it was, so its action can arrive here.
    fn on_verify_package(
        &mut self,
        _: &PropsVerifyPackage,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active == DetailTab::General
            && let Some(g) = &self.tabs.general
        {
            g.update(cx, |g, cx| g.verify_package(cx));
        }
    }

    fn on_close(&mut self, _: &PropsCloseWindow, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn on_escape(&mut self, _: &PropsEscape, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.filter[strip_index(self.active)].clone();
        let input = filter.read(cx).input.clone();
        let filter_focused_non_empty = {
            let state = input.read(cx);
            state.focus_handle(cx).contains_focused(window, cx)
                && !state.text().to_string().is_empty()
        };
        // The Strings scan (E23-021) adds its running state here.
        match esc_step(
            context_menu_open(window, cx),
            false,
            filter_focused_non_empty,
        ) {
            EscStep::ClosePopup => close_context_menu(window, cx),
            EscStep::CancelScan => {}
            EscStep::ClearFilter => filter.update(cx, |f, cx| f.clear(window, cx)),
            EscStep::CloseWindow => window.remove_window(),
        }
    }

    fn banner(&self, cx: &App) -> AnyElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let warning = theme.status.warning;
        let mut text = div().text_color(theme.text_primary);
        *text.text_style() = t_sm(&m);
        div()
            .debug_selector(|| "PropsExitedBanner".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[3])
            .w_full()
            .h(px(BANNER_H * s))
            .px(m.sp[5])
            .bg(warning.opacity(BANNER_FILL_ALPHA))
            .border_b_1()
            .border_color(warning.opacity(BANNER_BORDER_ALPHA))
            .child(
                Icon::new(IconName::CircleOff)
                    .size(px(BANNER_ICON * s))
                    .text_color(warning),
            )
            .child(text.child(self.words.exited_banner.clone()))
            .into_any_element()
    }

    fn body(&self) -> AnyElement {
        if self.active == DetailTab::General
            && let Some(general) = &self.tabs.general
        {
            return AnyView::from(general.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        let restricted = self
            .last_detail
            .as_ref()
            .is_some_and(|d| d.tab(self.active).validity == Validity::Restricted);
        match access_text(self.active).filter(|_| restricted) {
            Some(text) => access_block(text).into_any_element(),
            None => div().size_full().into_any_element(),
        }
    }
}

/// Position of `tab` in the strip. App tabs never show here and map to General.
fn strip_index(tab: DetailTab) -> usize {
    TABS.iter().position(|t| *t == tab).unwrap_or(0)
}

fn same_payload<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

impl gpui_kit::Render for PropsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let theme = current(cx).clone();
        let active = strip_index(self.active);
        let specs: Vec<TlTabSpec> = (0..TABS.len())
            .map(|i| {
                let spec = TlTabSpec::new(self.words.tabs[i].clone())
                    .tooltip(self.words.tab_tips[i].clone());
                match self.labels[i].0 {
                    Some(_) => spec.count(self.labels[i].1.clone()),
                    None => spec,
                }
            })
            .collect();
        let this = cx.weak_entity();
        let strip_id = ("props-tabs", window.window_handle().window_id().as_u64());
        let strip = TlTabs::new(strip_id)
            .tabs(specs)
            .selected(active)
            .on_change(move |ix, window, cx| {
                if let Some(&tab) = TABS.get(ix) {
                    let _ = this.update(cx, |t, cx| t.select_tab(tab, window, cx));
                }
            });
        let filter = &self.filter[active];

        div()
            .id("PropsWindow")
            .key_context(PROPS_WINDOW_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_select_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_focus_filter))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_verify_package))
            .on_action(cx.listener(Self::on_close))
            .on_action(cx.listener(Self::on_escape))
            .on_action(on_clear(filter))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(div().flex_none().w_full().child(strip))
            .child(filter_bar(filter, None, cx))
            .when(self.exited, |d| d.child(self.banner(cx)))
            .child(
                div()
                    .debug_selector(|| "PropsBody".into())
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(self.body()),
            )
    }
}
