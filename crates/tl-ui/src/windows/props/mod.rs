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
//! Every tab has its view. A tab whose payload is `Restricted` shows the access block instead.
//! Its Grant access or Enable button dispatches `StartHelperSession`, the shield's Action. When
//! the helper session becomes active the window sends one `DetailRefresh` for its active tab.

pub mod environ;
pub mod fds;
pub mod filter;
pub mod general;
pub mod maps;
pub mod security;
pub mod strings;
pub mod threads;

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::base::Root;
use gpui_kit::component::input::Escape;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::{
    AnyElement, AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, EntityId,
    FocusHandle, Focusable as _, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StyleRefinement, Styled as _, Subscription, Window, WindowHandle,
    div, prelude::FluentBuilder as _, px,
};
use tl_model::command::Command;
use tl_model::demand::{Demand, DetailDemand};
use tl_model::details::{DetailTab, DetailTarget, TabSlot, TargetDetail};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::ProcessKey;
use tl_model::process::{ProcFlags, ProcessRecord};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::system::CpuSet;
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use self::environ::{EnvCountChanged, EnvironTab};
use self::fds::{FdsCountChanged, FdsTab};
use self::filter::{PropsFilter, filter_bar, on_clear};
use self::general::GeneralTab;
use self::maps::{MapsCountChanged, MapsTab};
use self::security::SecurityTab;
use self::strings::StringsTab;
use self::threads::ThreadsTab;
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::menu::{close_context_menu, context_menu_open};
use crate::components::progress::TlSpinner;
use crate::components::shield::StartHelperSession;
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
    PROPS_WINDOW_CONTEXT, PropsCancelStringScan, PropsCloseWindow, PropsEscape, PropsFocusFilter,
    PropsNextTab, PropsPrevTab, PropsRefreshNow, PropsSelectTab, PropsStartStringScan,
    PropsVerifyPackage,
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
const KEY_ACCESS_GRANT: &str = "pwn-props-access-grant";
const KEY_ACCESS_ENABLE: &str = "pwn-props-access-enable";
const KEY_ACCESS_WAITING: &str = "pwn-props-access-waiting";
const KEY_ACCESS_NO_HELPER: &str = "pwn-props-access-no-helper";
const KEY_AUTH_CANCELLED: &str = "pwn-props-toast-auth-cancelled";
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
    pub access_grant: SharedString,
    pub access_enable: SharedString,
    pub access_waiting: SharedString,
    /// Tooltip of the disabled access button when the helper is not installed.
    pub access_no_helper: SharedString,
    pub auth_cancelled: SharedString,
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
            access_grant: t(KEY_ACCESS_GRANT),
            access_enable: t(KEY_ACCESS_ENABLE),
            access_waiting: t(KEY_ACCESS_WAITING),
            access_no_helper: t(KEY_ACCESS_NO_HELPER),
            auth_cancelled: t(KEY_AUTH_CANCELLED),
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

/// The button under the access block text (UI-PWN-018, P3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessButton {
    /// "Grant access": File descriptors, Memory maps and Environment.
    GrantAccess,
    /// "Enable": Strings.
    Enable,
}

/// The access button of `tab`. Security has none: only hidepid hides its world-readable files.
pub fn access_button(tab: DetailTab) -> Option<AccessButton> {
    match tab {
        DetailTab::Fds | DetailTab::Maps | DetailTab::Environment => {
            Some(AccessButton::GrantAccess)
        }
        DetailTab::Strings => Some(AccessButton::Enable),
        _ => None,
    }
}

/// What the access button shows for one helper state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessButtonView {
    pub kind: AccessButton,
    pub label: SharedString,
    pub disabled: bool,
    pub loading: bool,
    pub tooltip: Option<SharedString>,
}

/// The access button for `helper`. An exited process or a missing helper disables it; while the
/// authentication prompt is pending it shows "Waiting for authentication…" as loading.
pub fn access_button_view(
    kind: AccessButton,
    helper: HelperState,
    exited: bool,
    words: &PropsWords,
) -> AccessButtonView {
    let label = match kind {
        AccessButton::GrantAccess => words.access_grant.clone(),
        AccessButton::Enable => words.access_enable.clone(),
    };
    let view = AccessButtonView {
        kind,
        label,
        disabled: false,
        loading: false,
        tooltip: None,
    };
    if exited {
        return AccessButtonView {
            disabled: true,
            tooltip: Some(words.not_running.clone()),
            ..view
        };
    }
    match helper {
        HelperState::Missing => AccessButtonView {
            disabled: true,
            tooltip: Some(words.access_no_helper.clone()),
            ..view
        },
        HelperState::Starting => AccessButtonView {
            label: words.access_waiting.clone(),
            loading: true,
            ..view
        },
        HelperState::Locked | HelperState::Unlocked => view,
    }
}

/// The block a tab shows instead of its body when its payload is `Restricted`.
#[derive(IntoElement)]
pub struct AccessBlock {
    text: &'static str,
    button: Option<AccessButton>,
    view: Option<AccessButtonView>,
    focus: Option<FocusHandle>,
}

/// Centred 64 px under the body top: the `shield-alert` icon, then `text` (an `ACCESS_*`
/// constant), translated, then the button, if any. The button dispatches `StartHelperSession`
/// and never runs anything itself.
pub fn access_block(text: &'static str, button: Option<AccessButton>) -> AccessBlock {
    AccessBlock {
        text,
        button,
        view: None,
        focus: None,
    }
}

impl AccessBlock {
    /// The button state. Without it the button shows its default look.
    pub fn view(mut self, view: AccessButtonView) -> Self {
        self.view = Some(view);
        self
    }

    /// The owner's focus handle for the button, so the owner can focus it first.
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }
}

impl RenderOnce for AccessBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let words = props_words(cx);
        let text = words.access(self.text);
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let mut message = div()
            .debug_selector(|| "PropsAccessBlock-text".into())
            .max_w(px(ACCESS_MAX_W * s))
            .text_center()
            .text_color(theme.text_primary);
        *message.text_style() = t_base(&m);
        let button = self.button.map(|kind| {
            let view = self
                .view
                .unwrap_or_else(|| access_button_view(kind, HelperState::Locked, false, &words));
            // `TlButton::loading` hides the label, and "Waiting for authentication…" must stay
            // readable. While loading the button is inert and the spinner sits beside it.
            h_flex()
                .debug_selector(|| "PropsAccessButton".into())
                .mt(m.sp[5] - m.sp[4])
                .gap(m.sp[3])
                .child(
                    TlButton::new("PropsAccessButton", TlButtonKind::Secondary)
                        .size(TlButtonSize::Sm)
                        .label(view.label)
                        .action(Box::new(StartHelperSession))
                        .disabled(view.disabled || view.loading)
                        .when_some(view.tooltip, |b, tip| b.tooltip(tip))
                        .when_some(self.focus, |b, focus| b.focus_handle(focus)),
                )
                .when(view.loading, |row| {
                    row.child(
                        div()
                            .debug_selector(|| "PropsAccessButton-spinner".into())
                            .flex_none()
                            .child(TlSpinner::new(m.icon.sm)),
                    )
                })
        });
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
            .children(button)
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
    pub threads: Option<Entity<ThreadsTab>>,
    pub fds: Option<Entity<FdsTab>>,
    pub maps: Option<Entity<MapsTab>>,
    pub security: Option<Entity<SecurityTab>>,
    pub environ: Option<Entity<EnvironTab>>,
    pub strings: Option<Entity<StringsTab>>,
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
    /// The online CPUs, for a tab built later.
    online: CpuSet,
    words: Rc<PropsWords>,
    focus: FocusHandle,
    /// The helper session state from `Snapshot.health`.
    helper: HelperState,
    /// The last `helper_start` seq seen, so an outcome toasts at most once.
    start_seq: Option<u64>,
    /// The access button of this window started the helper and no outcome has come yet. Only
    /// then does a cancel toast here; the shield's own starts toast in the main window.
    grant_pending: bool,
    /// Focus handle of the access button, the body's first focus stop while it shows.
    access_focus: FocusHandle,
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
            online: CpuSet::empty(),
            words,
            focus: cx.focus_handle(),
            helper: HelperState::default(),
            start_seq: None,
            grant_pending: false,
            access_focus: cx.focus_handle(),
            demand: None,
            renders: 0,
            _subs: subs,
        };
        this.ensure_tab(DetailTab::General, window, cx);

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
            // An outcome from before the window opened never toasts here.
            this.helper = snap.health.helper;
            this.start_seq = snap.health.helper_start.as_ref().map(|(seq, _)| *seq);
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

    /// The helper session state this window last read.
    pub fn helper(&self) -> HelperState {
        self.helper
    }

    /// The access button the body shows now, or `None` when it shows none.
    pub fn access_button(&self) -> Option<AccessButtonView> {
        self.shown_access_button()
            .map(|kind| access_button_view(kind, self.helper, self.exited, &self.words))
    }

    /// The focus handle of the access button.
    pub fn access_focus(&self) -> &FocusHandle {
        &self.access_focus
    }

    /// The payload of the active tab is `Restricted`.
    fn active_restricted(&self) -> bool {
        self.last_detail
            .as_ref()
            .is_some_and(|d| d.tab(self.active).validity == Validity::Restricted)
    }

    /// The body shows the access block: the active tab is `Restricted` and the helper session
    /// is not active. Once it is, the tab shows its loading state until the helper read comes.
    fn shows_access_block(&self) -> bool {
        self.active != DetailTab::General
            && self.active_restricted()
            && self.helper != HelperState::Unlocked
    }

    fn shown_access_button(&self) -> Option<AccessButton> {
        access_button(self.active).filter(|_| self.shows_access_block())
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
        self.ensure_tab(tab, window, cx);
        self.register_demand(window, cx);
        self.focus_first(window, cx);
        cx.notify();
    }

    fn ensure_tab(&mut self, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.key;
        if tab == DetailTab::General && self.tabs.general.is_none() {
            let filter = self.filter[0].clone();
            self.tabs.general = Some(cx.new(|cx| GeneralTab::new(key, filter, cx)));
        }
        if tab == DetailTab::Threads && self.tabs.threads.is_none() {
            let filter = self.filter[strip_index(DetailTab::Threads)].clone();
            let threads = cx.new(|cx| ThreadsTab::new(key, filter, window, cx));
            let online = self.online;
            threads.update(cx, |t, cx| t.set_online_cpus(online, cx));
            if let Some(detail) = &self.last_detail {
                let payload = detail.tab(DetailTab::Threads).payload.clone();
                threads.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            self.tabs.threads = Some(threads);
        }
        if tab == DetailTab::Fds && self.tabs.fds.is_none() {
            let filter = self.filter[strip_index(DetailTab::Fds)].clone();
            let fds = cx.new(|cx| FdsTab::new(key, filter, window, cx));
            // The hide switch lives in the filter bar, so a change re-renders the window too.
            self._subs
                .push(cx.subscribe(&fds, |this, _, ev: &FdsCountChanged, cx| {
                    this.set_count(DetailTab::Fds, ev.0, cx);
                    cx.notify();
                }));
            if let Some(detail) = &self.last_detail {
                let payload = detail.tab(DetailTab::Fds).payload.clone();
                fds.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if self.exited {
                fds.update(cx, |t, cx| t.set_exited(true, cx));
            }
            self.tabs.fds = Some(fds);
        }
        if tab == DetailTab::Maps && self.tabs.maps.is_none() {
            let filter = self.filter[strip_index(DetailTab::Maps)].clone();
            let maps = cx.new(|cx| MapsTab::new(key, filter, window, cx));
            // The switch lives in the filter bar, so a change re-renders the window too.
            self._subs
                .push(cx.subscribe(&maps, |this, _, ev: &MapsCountChanged, cx| {
                    this.set_count(DetailTab::Maps, ev.0, cx);
                    cx.notify();
                }));
            if let Some(detail) = &self.last_detail {
                let payload = detail.tab(DetailTab::Maps).payload.clone();
                maps.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if self.exited {
                maps.update(cx, |t, cx| t.set_exited(true, cx));
            }
            self.tabs.maps = Some(maps);
        }
        if tab == DetailTab::Security && self.tabs.security.is_none() {
            let filter = self.filter[strip_index(DetailTab::Security)].clone();
            let security = cx.new(|cx| SecurityTab::new(key, filter, cx));
            if let Some(detail) = &self.last_detail {
                let payload = detail.tab(DetailTab::Security).payload.clone();
                security.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if self.exited {
                security.update(cx, |t, cx| t.set_exited(true, cx));
            }
            self.tabs.security = Some(security);
        }
        if tab == DetailTab::Environment && self.tabs.environ.is_none() {
            let filter = self.filter[strip_index(DetailTab::Environment)].clone();
            let environ = cx.new(|cx| EnvironTab::new(key, filter, window, cx));
            // Refresh lives in the filter bar, so a new payload re-renders the window too.
            self._subs
                .push(cx.subscribe(&environ, |this, _, ev: &EnvCountChanged, cx| {
                    this.set_count(DetailTab::Environment, ev.0, cx);
                    cx.notify();
                }));
            if let Some(detail) = &self.last_detail {
                let payload = detail.tab(DetailTab::Environment).payload.clone();
                environ.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if self.exited {
                environ.update(cx, |t, cx| t.set_exited(true, cx));
            }
            self.tabs.environ = Some(environ);
        }
        if tab == DetailTab::Strings && self.tabs.strings.is_none() {
            let filter = self.filter[strip_index(DetailTab::Strings)].clone();
            let strings = cx.new(|cx| StringsTab::new(key, filter, window, cx));
            let kernel_thread = self
                .last_record
                .as_ref()
                .is_some_and(|r| r.flags.contains(ProcFlags::KERNEL_THREAD));
            strings.update(cx, |t, cx| t.set_kernel_thread(kernel_thread, cx));
            if self.exited {
                strings.update(cx, |t, cx| t.set_exited(true, cx));
            }
            self.tabs.strings = Some(strings);
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

    /// Focuses the active tab's first focusable element: the access button while the block
    /// shows it, the filter for General and Security, the table for Threads, else the window
    /// root until the tab has its view.
    fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.access_button() {
            if view.disabled {
                self.focus.focus(window, cx);
            } else {
                self.access_focus.focus(window, cx);
            }
            return;
        }
        if self.shows_access_block() {
            // Security: the block has no button, so the filter comes first.
            self.focus_filter(window, cx);
            return;
        }
        match self.active {
            DetailTab::General | DetailTab::Security => self.focus_filter(window, cx),
            DetailTab::Threads => match &self.tabs.threads {
                Some(t) => {
                    let focus = t.read(cx).table().read(cx).focus_handle().clone();
                    focus.focus(window, cx);
                }
                None => self.focus.focus(window, cx),
            },
            DetailTab::Fds => match &self.tabs.fds {
                Some(t) => {
                    let focus = t.read(cx).table().read(cx).focus_handle().clone();
                    focus.focus(window, cx);
                }
                None => self.focus.focus(window, cx),
            },
            DetailTab::Maps => match &self.tabs.maps {
                Some(t) => {
                    let focus = t.read(cx).table().read(cx).focus_handle().clone();
                    focus.focus(window, cx);
                }
                None => self.focus.focus(window, cx),
            },
            DetailTab::Environment => match &self.tabs.environ {
                Some(t) => {
                    let focus = t.read(cx).table().read(cx).focus_handle().clone();
                    focus.focus(window, cx);
                }
                None => self.focus.focus(window, cx),
            },
            DetailTab::Strings => match &self.tabs.strings {
                Some(t) => {
                    let focus = t.read(cx).options_focus().clone();
                    focus.focus(window, cx);
                }
                None => self.focus.focus(window, cx),
            },
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
        let button_before = self.access_button().map(|v| v.disabled);
        self.take_snapshot_inner(snap, mask, window, cx);
        // The block came or went, or the button was enabled or disabled: move focus to the new
        // first stop, unless the user is typing in the filter.
        if button_before != self.access_button().map(|v| v.disabled) {
            let input = self.filter[strip_index(self.active)].read(cx).input.clone();
            let typing = input.read(cx).focus_handle(cx).contains_focused(window, cx);
            if !typing {
                self.focus_first(window, cx);
            }
        }
    }

    fn take_snapshot_inner(
        &mut self,
        snap: &Snapshot,
        mask: ComponentMask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mask.intersects(ComponentMask::HEALTH) {
            self.take_health(snap, window, cx);
        }
        if self.exited {
            return;
        }
        let general = self.tabs.general.clone();
        let threads = self.tabs.threads.clone();
        let fds = self.tabs.fds.clone();
        let maps = self.tabs.maps.clone();
        let security = self.tabs.security.clone();
        let environ = self.tabs.environ.clone();
        let strings = self.tabs.strings.clone();
        if mask.intersects(ComponentMask::SYSTEM) {
            self.mem_total = snap.system.memory.total;
            let online = snap.system.cpu_topology.online;
            self.online = online;
            if let Some(g) = &general {
                g.update(cx, |g, cx| g.set_online_cpus(online, cx));
            }
            if let Some(t) = &threads {
                t.update(cx, |t, cx| t.set_online_cpus(online, cx));
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
                    if let Some(t) = &strings {
                        let kernel_thread = record.flags.contains(ProcFlags::KERNEL_THREAD);
                        t.update(cx, |t, cx| t.set_kernel_thread(kernel_thread, cx));
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
            let threads_changed = changed(DetailTab::Threads);
            let fds_changed = changed(DetailTab::Fds);
            let maps_changed = changed(DetailTab::Maps);
            let security_changed = changed(DetailTab::Security);
            let environ_changed = changed(DetailTab::Environment);
            let strings_changed = changed(DetailTab::Strings);
            let active_changed = changed(self.active);
            let validity_changed = self.last_detail.as_ref().is_none_or(|last| {
                last.tab(self.active).validity != detail.tab(self.active).validity
            });
            if !(general_changed
                || threads_changed
                || fds_changed
                || maps_changed
                || security_changed
                || environ_changed
                || strings_changed
                || active_changed
                || validity_changed)
            {
                return;
            }
            if fds_changed && let Some(t) = &fds {
                let payload = detail.tab(DetailTab::Fds).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if environ_changed && let Some(t) = &environ {
                let payload = detail.tab(DetailTab::Environment).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if strings_changed && let Some(t) = &strings {
                let payload = detail.tab(DetailTab::Strings).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), window, cx));
            }
            if security_changed && let Some(t) = &security {
                let payload = detail.tab(DetailTab::Security).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if maps_changed && let Some(t) = &maps {
                let payload = detail.tab(DetailTab::Maps).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
            }
            if threads_changed && let Some(t) = &threads {
                let payload = detail.tab(DetailTab::Threads).payload.clone();
                t.update(cx, |t, cx| t.set_payload(payload.as_ref(), cx));
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

    /// Reads the helper session state (UI-PWN-018). A cancel of this window's own start toasts
    /// once per `helper_start` seq. When the session becomes active and the active tab or
    /// General is `Restricted`, one `DetailRefresh` for the active tab re-reads it through the
    /// helper.
    fn take_health(&mut self, snap: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let health = &snap.health;
        if let Some((seq, outcome)) = &health.helper_start
            && self.start_seq != Some(*seq)
        {
            self.start_seq = Some(*seq);
            if std::mem::take(&mut self.grant_pending) && *outcome == HelperStartOutcome::Cancelled
            {
                let text = self.words.auth_cancelled.clone();
                if let Some(queue) = ToastQueue::of_window(window, cx) {
                    queue.update(cx, |q, cx| q.push(ToastKind::Info, text, None, None, cx));
                }
            }
        }
        let helper = health.helper;
        if helper == self.helper {
            return;
        }
        let unlocked = helper == HelperState::Unlocked && self.helper != HelperState::Unlocked;
        self.helper = helper;
        if helper == HelperState::Unlocked {
            self.grant_pending = false;
        }
        let restricted = |tab| {
            self.last_detail
                .as_ref()
                .is_some_and(|d| d.tab(tab).validity == Validity::Restricted)
        };
        if unlocked && !self.exited && (restricted(self.active) || restricted(DetailTab::General)) {
            self.send(
                Command::DetailRefresh {
                    target: DetailTarget::Process(self.key),
                    tab: self.active,
                },
                cx,
            );
        }
        cx.notify();
    }

    /// The access button started the helper: a cancel of this start toasts here.
    fn on_start_helper(&mut self, _: &StartHelperSession, _: &mut Window, _: &mut Context<Self>) {
        if !self.exited && self.helper == HelperState::Locked {
            self.grant_pending = true;
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
        if let Some(t) = &self.tabs.threads {
            t.update(cx, |t, cx| t.set_exited(true, cx));
        }
        if let Some(t) = &self.tabs.fds {
            t.update(cx, |t, cx| t.set_exited(true, cx));
        }
        if let Some(t) = &self.tabs.maps {
            t.update(cx, |t, cx| t.set_exited(true, cx));
        }
        if let Some(t) = &self.tabs.security {
            t.update(cx, |t, cx| t.set_exited(true, cx));
        }
        if let Some(t) = &self.tabs.environ {
            t.update(cx, |t, cx| t.set_exited(true, cx));
        }
        if let Some(t) = &self.tabs.strings {
            t.update(cx, |t, cx| t.set_exited(true, cx));
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
        // The Environment tab also masks every row again and shows Refresh as loading.
        if self.active == DetailTab::Environment
            && let Some(t) = &self.tabs.environ
        {
            t.update(cx, |t, cx| t.refresh(cx));
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
        match esc_step(
            context_menu_open(window, cx),
            self.string_scan_running(cx),
            filter_focused_non_empty,
        ) {
            EscStep::ClosePopup => close_context_menu(window, cx),
            EscStep::CancelScan => self.cancel_string_scan(cx),
            EscStep::ClearFilter => filter.update(cx, |f, cx| f.clear(window, cx)),
            EscStep::CloseWindow => window.remove_window(),
        }
    }

    /// The Strings tab is active and its scan runs: Esc cancels it first.
    pub fn string_scan_running(&self, cx: &App) -> bool {
        self.active == DetailTab::Strings
            && self
                .tabs
                .strings
                .as_ref()
                .is_some_and(|t| t.read(cx).scan_running())
    }

    fn cancel_string_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = &self.tabs.strings {
            t.update(cx, |t, cx| t.cancel_scan(cx));
        }
    }

    /// The Escape of a focused text field. A filter with text would clear itself first, so a
    /// running scan is cancelled here, in the capture phase (UI-PWN-001 Esc order).
    fn on_field_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if !context_menu_open(window, cx) && self.string_scan_running(cx) {
            self.cancel_string_scan(cx);
            cx.stop_propagation();
        }
    }

    /// Scan: the button, or Enter in the options bar. A restricted tab shows no options.
    fn on_start_string_scan(
        &mut self,
        _: &PropsStartStringScan,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let restricted = self
            .last_detail
            .as_ref()
            .is_some_and(|d| d.tab(DetailTab::Strings).validity == Validity::Restricted);
        if self.active != DetailTab::Strings || self.exited || restricted {
            return;
        }
        if let Some(t) = &self.tabs.strings {
            t.update(cx, |t, cx| t.start_scan(cx));
        }
    }

    fn on_cancel_string_scan(
        &mut self,
        _: &PropsCancelStringScan,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_string_scan(cx);
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
        // With the helper session active a Restricted tab shows its view, which shows its
        // loading state until the helper read comes.
        let restricted_now = self.shows_access_block();
        if self.active == DetailTab::Threads
            && !restricted_now
            && let Some(threads) = &self.tabs.threads
        {
            return AnyView::from(threads.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        if self.active == DetailTab::Fds
            && !restricted_now
            && let Some(fds) = &self.tabs.fds
        {
            return AnyView::from(fds.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        if self.active == DetailTab::Security
            && !restricted_now
            && let Some(security) = &self.tabs.security
        {
            return AnyView::from(security.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        if self.active == DetailTab::Environment
            && !restricted_now
            && let Some(environ) = &self.tabs.environ
        {
            return AnyView::from(environ.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        if self.active == DetailTab::Maps
            && !restricted_now
            && let Some(maps) = &self.tabs.maps
        {
            return AnyView::from(maps.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        if self.active == DetailTab::Strings
            && !restricted_now
            && let Some(strings) = &self.tabs.strings
        {
            return AnyView::from(strings.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element();
        }
        match access_text(self.active).filter(|_| restricted_now) {
            Some(text) => {
                let mut block =
                    access_block(text, access_button(self.active)).focus(self.access_focus.clone());
                if let Some(view) = self.access_button() {
                    block = block.view(view);
                }
                block.into_any_element()
            }
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
        let tools = match (&self.tabs.fds, self.active) {
            (Some(fds), DetailTab::Fds) => Some(fds.read(cx).filter_tool(cx)),
            _ => match (&self.tabs.maps, self.active) {
                (Some(maps), DetailTab::Maps) => Some(maps.read(cx).filter_tool(cx)),
                _ => match (&self.tabs.environ, self.active) {
                    (Some(environ), DetailTab::Environment) => {
                        Some(environ.read(cx).filter_tool(cx))
                    }
                    _ => None,
                },
            },
        };

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
            .on_action(cx.listener(Self::on_start_string_scan))
            .on_action(cx.listener(Self::on_cancel_string_scan))
            .capture_action(cx.listener(Self::on_field_escape))
            .capture_action(cx.listener(Self::on_start_helper))
            .on_action(on_clear(filter))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(div().flex_none().w_full().child(strip))
            .child(filter_bar(filter, tools, cx))
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
