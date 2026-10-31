//! "tasklens - Low memory" rescue window (UI-PWN-032, UI-PWN-033, PROC-098).
//!
//! The window is its own process, `tasklens --low-memory`, started by the watcher.
//! `init_low_memory` replaces `TasklensUi::init` there (R-243): two font faces, no icon loading,
//! only the `LowMemory` key bindings and no pages. The window shows two warning lines, a filter,
//! the process list (E32-A2) and "Open Full tasklens". When the pressure has stayed below the
//! threshold for 10 s, the two lines become one "eased" line; a critical sample brings them back.
//!
//! The title follows the language: it changes when the strings version changes.
//!
//! The window has no `WindowKey`, so the frame dispatches no `ToggleAlwaysOnTop` for it. The
//! window starts pinned and sends `KWinCommand::KeepAbove` itself whenever the frame's pin flips
//! (R-073, R-152 OI-9). Closing the window ends the process.

use std::borrow::Cow;
use std::cmp::{Ordering as CmpOrdering, Reverse};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, AsyncApp, Context, Entity, FocusHandle, FontWeight, Global,
    Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render,
    SharedString, StatefulInteractiveElement as _, Styled, Subscription, WeakEntity, Window,
    WindowHandle, div, px, size,
};
use smallvec::{SmallVec, smallvec};
use tl_model::actions::{
    ActionId, ActionKind, ActionRequest, ActionTarget, LaunchSpec, LaunchWhat, Signal,
};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand};
use tl_model::ids::{GroupKey, ProcessKey};
use tl_model::process::{LowMemGroup, LowMemoryInfo, ProcFlags, ProcessRecord};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::time::TimeNs;
use tl_model::validity::{ProcessField, Validity};
use tracing::{debug, warn};

use crate::action_tracker::{ActionTrackerEvent, ToastTexts, reconnecting_refusal};
use crate::app::{
    AppRequestHandler, BootSettings, EngineGlobal, OpenHooks, UiExit, UiInit, request_exit,
};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::input::{TlFilterBox, TlInputSize, TlTextInput};
use crate::components::toast::{self, ToastQueue};
use crate::components::validity::glyph;
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::host::HostContext;
use crate::hub::{GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{self, Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, numeric_style, t_base};
use crate::theme::user_store::ThemePersist;
use crate::theme::{self, TlTheme, UiMetrics, current, metrics};

gpui_kit::actions!(
    tasklens,
    [
        /// Ends the highlighted row's app or process (E32-A2).
        LowMemEndTask,
        /// Starts the main tasklens instance; the rescue window stays open.
        OpenFullTasklens,
        /// Closes the window and ends the `--low-memory` process (Ctrl+W).
        LowMemClose,
        /// Clears the filter when it has text, else closes the window (Esc).
        LowMemClearOrClose
    ]
);

/// The key context of the window content (table (b)).
pub const CONTEXT: &str = "LowMemory";

/// Default and minimum window size, × ui_scale.
pub const SIZE: (f32, f32) = (420., 560.);
pub const MIN_SIZE: (f32, f32) = (360., 400.);
/// Footer height, × ui_scale.
const FOOTER: f32 = 52.;
/// List row height, × ui_scale, not affected by the row-height setting.
const ROW: f32 = 32.;
/// Width of the "Memory 12.0%" text, × ui_scale.
const MEMORY_W: f32 = 100.;
/// Width of the End Task button, × ui_scale.
const BUTTON_W: f32 = 84.;
/// The list shows at most this many apps and processes (PROC-098).
pub const MAX_ROWS: usize = 30;

/// How long every sample must stay below the threshold before the eased line shows.
pub const EASE_AFTER_NS: TimeNs = 10_000_000_000;
/// A sample is below the threshold when the PSI full share is at most this.
pub const PSI_FULL_MAX_PCT: f32 = 10.;
/// ...and MemAvailable is at least this percent of MemTotal.
pub const AVAILABLE_MIN_PCT: u64 = 3;

/// The program "Open Full tasklens" starts. A second instance activates the running one and
/// exits (APP-011).
const MAIN_PROGRAM: &str = "tasklens";

/// The two faces the rescue process loads (R-243).
const FACES: [&[u8]; 2] = [
    include_bytes!("../../../../assets/fonts/NotoSans-Regular.ttf"),
    include_bytes!("../../../../assets/fonts/NotoSans-SemiBold.ttf"),
];

const KEY_TITLE: &str = "lowmem-window-title";
const KEY_CRITICAL: &str = "lowmem-text-critical";
const KEY_RECOMMEND: &str = "lowmem-text-recommend";
const KEY_EASED: &str = "lowmem-text-eased";
const KEY_FILTER_PLACEHOLDER: &str = "lowmem-filter-placeholder";
const KEY_FILTER_EMPTY: &str = "lowmem-filter-empty";
const KEY_OPEN_FULL: &str = "lowmem-button-open-full";
const KEY_ROW_MEMORY: &str = "lowmem-row-memory";
const KEY_END_TASK: &str = "lowmem-button-end-task";
const KEY_END_TASK_TOOLTIP: &str = "lowmem-tooltip-end-task";
const KEY_NEEDS_ADMIN: &str = "lowmem-tooltip-needs-admin";
const KEY_END_TASK_A11Y: &str = "lowmem-a11y-end-task";
const KEY_END_FAILED: &str = "lowmem-toast-end-failed";

/// Binds Esc and Ctrl+W in `LowMemory`. The only key binding call in the rescue process.
pub fn bind_keys_low_memory(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", LowMemClearOrClose, Some(CONTEXT)),
        KeyBinding::new("ctrl-w", LowMemClose, Some(CONTEXT)),
    ]);
}

/// The minimal UI init for the rescue process (R-243). `tl_ui::run` calls it instead of
/// `TasklensUi::init` when `UiLaunch.open == OpenMode::LowMemory` (E32-013). Opens no window;
/// it installs the hook `open_first` calls.
pub fn init_low_memory(cx: &mut App, client: Arc<dyn EngineClient>, init: UiInit) {
    let UiInit {
        settings,
        state,
        themes_json: _,
        persist,
        persist_themes,
        app_requests,
        startup_toasts,
        env,
    } = init;

    // 1. Fonts: Noto Sans Regular and SemiBold only.
    let faces = FACES.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(err) = cx.text_system().add_fonts(faces) {
        warn!(target: "tasklens::ui", error = %err, "rescue fonts could not be added");
    }

    // 2. Kit.
    gpui_kit::init(cx);

    // 4. Theme without user themes.
    cx.set_global(ThemePersist(persist_themes));
    theme::store::init(cx, &settings.theme, &settings.appearance, Vec::new());

    // 5. Strings.
    i18n::init_strings(cx, &settings.general.language);

    // 6. Engine.
    cx.set_global(EngineGlobal(client.clone()));

    // 7. Hub. No demand: the low-memory engine runs a fixed sweep.
    let hub = cx.new(|cx| SnapshotHub::new(client.clone(), cx));
    cx.set_global(GlobalHub(hub.clone()));

    // 8. Boot settings and the tl-app environment.
    cx.set_global(BootSettings {
        settings,
        state,
        persist: Some(persist),
        startup_toasts,
    });
    TlSettings::init(cx);
    cx.set_global(env);

    // 9. AppRequest loop.
    cx.set_global(AppRequestHandler(Box::new(|req, _| {
        debug!(target: "tasklens::ui", ?req, "app request");
    })));
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(req) = app_requests.recv().await {
            cx.update_global::<AppRequestHandler, _>(|h, cx| (h.0)(req, cx));
        }
    })
    .detach();

    bind_keys_low_memory(cx);
    // Before any window, so the Root mounts the toast layer.
    toast::init(cx);

    // ActionTracker
    crate::action_tracker::install(client.clone(), Some(hub), cx);

    cx.default_global::<OpenHooks>().low_memory = Some(Box::new(move |cx| {
        if let Err(err) = open_first_rescue(cx, client.clone()) {
            warn!(target: "tasklens::ui", %err, "rescue window did not open");
            request_exit(cx, UiExit::Quit);
        }
    }));
}

/// Makes the rescue window open from the low-memory hook fail with this text, for the
/// open-failure tests.
#[cfg(any(test, feature = "test-hooks"))]
pub struct FailLowMemoryOpen(pub String);

#[cfg(any(test, feature = "test-hooks"))]
impl Global for FailLowMemoryOpen {}

/// Opens the rescue window for the low-memory hook. The error text on failure.
fn open_first_rescue(cx: &mut App, engine: Arc<dyn EngineClient>) -> Result<(), String> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(fail) = cx.try_global::<FailLowMemoryOpen>() {
        return Err(fail.0.clone());
    }
    LowMemoryWindow::open(cx, engine)
        .map(drop)
        .map_err(|err| err.to_string())
}

/// The window texts, translated once per strings version by [`low_mem_words`].
#[derive(Clone, Debug)]
pub struct LowMemWords {
    pub title: SharedString,
    pub critical: SharedString,
    pub recommend: SharedString,
    pub eased: SharedString,
    pub filter_placeholder: SharedString,
    pub filter_empty: SharedString,
    pub open_full: SharedString,
    /// "Memory { $pct }%".
    pub row_memory: Template,
    pub end_task: SharedString,
    pub end_task_tooltip: SharedString,
    pub needs_admin: SharedString,
    /// "End Task: { $name }".
    pub end_task_a11y: Template,
    /// The failure prefix; the tracker adds the error text after it.
    pub end_failed: SharedString,
}

impl LowMemWords {
    fn with(t: impl Fn(&str) -> SharedString, tmpl: impl Fn(&str, &[&str]) -> Template) -> Self {
        Self {
            title: t(KEY_TITLE),
            critical: t(KEY_CRITICAL),
            recommend: t(KEY_RECOMMEND),
            eased: t(KEY_EASED),
            filter_placeholder: t(KEY_FILTER_PLACEHOLDER),
            filter_empty: t(KEY_FILTER_EMPTY),
            open_full: t(KEY_OPEN_FULL),
            row_memory: tmpl(KEY_ROW_MEMORY, &["pct"]),
            end_task: t(KEY_END_TASK),
            end_task_tooltip: t(KEY_END_TASK_TOOLTIP),
            needs_admin: t(KEY_NEEDS_ADMIN),
            end_task_a11y: tmpl(KEY_END_TASK_A11Y, &["name"]),
            end_failed: t(KEY_END_FAILED),
        }
    }
}

struct LowMemWordsCache {
    version: u64,
    words: Rc<LowMemWords>,
}

impl Global for LowMemWordsCache {}

/// The texts for the current strings. Translated only when the strings version changed since
/// the last call, so a frame makes no `tr` call. Without strings they are in en-US.
pub fn low_mem_words(cx: &mut App) -> Rc<LowMemWords> {
    if let Some(words) = cached_words(cx) {
        return words;
    }
    let words = Rc::new(fresh_words(cx));
    if let Some(version) = cx.try_global::<Strings>().map(|s| s.strings_version) {
        cx.set_global(LowMemWordsCache {
            version,
            words: words.clone(),
        });
    }
    words
}

/// The cached texts when they belong to the current strings version.
fn cached_words(cx: &App) -> Option<Rc<LowMemWords>> {
    let version = cx.try_global::<Strings>()?.strings_version;
    cx.try_global::<LowMemWordsCache>()
        .filter(|cache| cache.version == version)
        .map(|cache| cache.words.clone())
}

fn fresh_words(cx: &App) -> LowMemWords {
    if cx.has_global::<Strings>() {
        LowMemWords::with(|key| tr(cx, key), |key, args| Template::new(cx, key, args))
    } else {
        LowMemWords::with(|key| tr_en(key, &[]), Template::en)
    }
}

/// [`low_mem_words`] without filling the cache.
fn words_now(cx: &App) -> Rc<LowMemWords> {
    cached_words(cx).unwrap_or_else(|| Rc::new(fresh_words(cx)))
}

/// Whether one sample is below the threshold: the PSI full share at most 10% (a missing share
/// counts as below) and MemAvailable at least 3% of MemTotal.
pub fn below_threshold(info: &LowMemoryInfo) -> bool {
    let psi_ok = info.psi_full_pct.is_none_or(|p| p <= PSI_FULL_MAX_PCT);
    let available_ok =
        info.mem_available.saturating_mul(100) >= info.mem_total.saturating_mul(AVAILABLE_MIN_PCT);
    psi_ok && available_ok
}

/// The eased state from the pressure samples. Holds the time of the first sample of the current
/// run of samples below the threshold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EaseTracker {
    calm_since: Option<TimeNs>,
    eased: bool,
}

impl EaseTracker {
    /// Adds one sample taken at `at`. Eased once every sample of the last 10 s was below the
    /// threshold; a sample above it ends the eased state at once.
    pub fn sample(&mut self, at: TimeNs, below: bool) {
        self.eased = if below {
            let since = *self.calm_since.get_or_insert(at);
            at.saturating_sub(since) >= EASE_AFTER_NS
        } else {
            self.calm_since = None;
            false
        };
    }

    pub fn eased(&self) -> bool {
        self.eased
    }
}

/// One line at the top of the window.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderLine {
    pub text: SharedString,
    pub color: Hsla,
    /// `t_md` 14 SemiBold; else `t_base`.
    pub strong: bool,
}

/// What one list row ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
    Process(ProcessKey),
    App(GroupKey),
}

/// One list row (UI-PWN-033). Its texts are built when its name or percent changes, never per
/// frame.
#[derive(Clone, Debug)]
pub struct LowMemRow {
    pub id: RowId,
    /// The target End Task sends, as captured from the sweep: the process key, or the app
    /// cgroup with its inode. Never looked up again by pid.
    pub target: ActionTarget,
    /// Apps: "{app name} ({process count})". Processes: the display name.
    pub name: SharedString,
    /// RssAnon plus swap for a process, `memory.current` for an app.
    pub memory_bytes: u64,
    pub validity: Validity,
    /// "Memory 12.0%". Empty when the memory is not `Valid`.
    pub memory_text: SharedString,
    /// "End Task: {name}".
    pub a11y: SharedString,
    /// Another user's process or app: End Task is disabled.
    pub other_user: bool,
    /// Percent of MemTotal in tenths, so the text is rebuilt only when it changes.
    tenths: u32,
    /// The name as the sweep gave it and the app's process count.
    name_src: Arc<str>,
    count: u32,
}

/// Where a row's values come from in the current sweep.
#[derive(Clone, Copy)]
enum RowSrc<'a> {
    App(&'a LowMemGroup),
    Process(&'a ProcessRecord),
}

/// One row before the 30-row cut: an index into the groups or the records.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    valid: bool,
    memory: u64,
    /// 0 for an app, 1 for a process, then the index, so ties keep the sweep order.
    src: (u8, u32),
}

impl Candidate {
    /// `Valid` memory first, then memory descending.
    fn order(a: &Self, b: &Self) -> CmpOrdering {
        (Reverse(a.valid), Reverse(a.memory), a.src).cmp(&(
            Reverse(b.valid),
            Reverse(b.memory),
            b.src,
        ))
    }
}

/// RssAnon plus swap, and its validity: the RssAnon validity. Swap counts only when `Valid`.
fn process_memory(r: &ProcessRecord) -> (Validity, u64) {
    let validity = r.validity.get(ProcessField::RssAnon);
    if !validity.is_valid() {
        return (validity, 0);
    }
    let swap = if r.validity.get(ProcessField::Swap).is_valid() {
        r.swap_bytes
    } else {
        0
    };
    (Validity::Valid, r.rss_anon_bytes.saturating_add(swap))
}

/// `memory` as a percent of `total` in tenths, rounded half up. 0 when `total` is 0.
fn tenths(memory: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    let t = (u128::from(memory) * 1000 + u128::from(total) / 2) / u128::from(total);
    u32::try_from(t).unwrap_or(u32::MAX)
}

/// The uid of this session: the uid of any record not marked `OTHER_USER`.
fn session_uid(records: &[ProcessRecord]) -> Option<u32> {
    records
        .iter()
        .find(|r| !r.flags.contains(ProcFlags::OTHER_USER))
        .map(|r| r.uid)
}

/// The row of `src`. Texts are taken from `prev` when it is the same row with the same name,
/// count, percent and validity.
fn make_row(
    src: RowSrc<'_>,
    mem_total: u64,
    uid: Option<u32>,
    words: &LowMemWords,
    prev: Option<&LowMemRow>,
) -> LowMemRow {
    let (id, target, name_src, count, other_user, validity, memory_bytes) = match src {
        RowSrc::App(g) => (
            RowId::App(g.group),
            ActionTarget::Cgroup(g.cgroup.clone()),
            g.name.clone(),
            g.process_count,
            uid.is_some_and(|uid| uid != g.uid),
            Validity::Valid,
            g.memory_current,
        ),
        RowSrc::Process(r) => {
            let (validity, memory) = process_memory(r);
            (
                RowId::Process(r.key),
                ActionTarget::Process(r.key),
                r.meta.name.clone(),
                0,
                r.flags.contains(ProcFlags::OTHER_USER),
                validity,
                memory,
            )
        }
    };
    let tenths = tenths(memory_bytes, mem_total);
    let prev = prev.filter(|p| p.id == id);
    let (name, a11y) = match prev {
        Some(p) if p.count == count && p.name_src == name_src => (p.name.clone(), p.a11y.clone()),
        _ => {
            let name: SharedString = match id {
                RowId::App(_) => format!("{name_src} ({count})").into(),
                RowId::Process(_) => name_src.clone().into(),
            };
            let mut a11y = String::new();
            let _ = words.end_task_a11y.write(&mut a11y, &[&name]);
            (name, a11y.into())
        }
    };
    let memory_text = match prev {
        Some(p) if p.tenths == tenths && p.validity == validity => p.memory_text.clone(),
        _ if validity.is_valid() => {
            let mut text = String::new();
            words.row_memory.write_each(&mut text, |_, out| {
                let _ = write!(out, "{}.{}", tenths / 10, tenths % 10);
            });
            text.into()
        }
        _ => SharedString::default(),
    };
    LowMemRow {
        id,
        target,
        name,
        memory_bytes,
        validity,
        memory_text,
        a11y,
        other_user,
        tenths,
        name_src,
        count,
    }
}

/// "Needs administrator rights" on another user's row, else "Ends immediately, unsaved work is
/// lost".
fn end_task_tooltip_in(row: &LowMemRow, words: &LowMemWords) -> SharedString {
    if row.other_user {
        words.needs_admin.clone()
    } else {
        words.end_task_tooltip.clone()
    }
}

/// The End Task request for `target`: SIGKILL for a process, a `cgroup.kill` write for an app
/// (ADR-006). No confirmation (PROC-098).
fn end_task_request(target: &ActionTarget) -> ActionRequest {
    let kind = match target {
        ActionTarget::Cgroup(_) => ActionKind::KillGroup,
        _ => ActionKind::Signal(Signal::Kill),
    };
    ActionRequest {
        id: ActionId(0),
        kind,
        targets: smallvec![target.clone()],
        privileged: false,
    }
}

/// Action ids come from the UI, counting up from 1 (ADR-015).
fn next_action_id() -> ActionId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ActionId(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn send_keep_above(engine: &dyn EngineClient, caption: &SharedString, on: bool) {
    let cmd = Command::KWin(KWinCommand::KeepAbove {
        pid: std::process::id(),
        caption: Some(Arc::from(caption.as_ref())),
        on,
    });
    if let Err(err) = engine.send(cmd) {
        warn!(target: "tasklens::ui", ?err, on, "keep-above not sent");
    }
}

/// The rescue window content. The frame around it is `TlSecondaryWindow`.
pub struct LowMemoryWindow {
    /// The host this window shows: the local host. Set when it opens; never changes.
    host: HostContext,
    engine: Arc<dyn EngineClient>,
    filter: Entity<InputState>,
    /// The list is one focus stop.
    list_focus: FocusHandle,
    ease: EaseTracker,
    /// The last sample taken, compared by pointer so a sweep counts once.
    last_info: Option<Arc<LowMemoryInfo>>,
    /// The records of that sweep, sorted by `ProcessKey`.
    records: Arc<[ProcessRecord]>,
    /// At most 30 rows, in the order shown.
    rows: Vec<LowMemRow>,
    /// The previous rows, kept for their allocation.
    spare: Vec<LowMemRow>,
    candidates: Vec<Candidate>,
    /// The words the row texts were built with.
    rows_words: Option<Rc<LowMemWords>>,
    /// The lowercased filter text.
    needle: String,
    /// The pointer is over the list.
    hovered: bool,
    /// The keyboard highlight (`row_selected`).
    highlight: Option<RowId>,
    /// End Task requests in flight; their buttons show loading.
    ending: SmallVec<[(ActionId, RowId); 2]>,
    _subscriptions: SmallVec<[Subscription; 6]>,
}

impl LowMemoryWindow {
    /// Opens the window through `TlSecondaryWindow::open` as a floating tool window with no
    /// parent, pinned. Returns the open error and opens nothing on failure.
    pub fn open(
        cx: &mut App,
        engine: Arc<dyn EngineClient>,
    ) -> gpui_kit::Result<WindowHandle<Root>> {
        let s = metrics(cx).ui_scale;
        let title = low_mem_words(cx).title.clone();
        let spec = TlWindowSpec::new(
            TlWindowKind::Tool { resizable: true },
            title.clone(),
            size(px(SIZE.0 * s), px(SIZE.1 * s)),
        )
        .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)));
        let content_engine = engine.clone();
        // The rescue window is not opened from a host: it shows the local host.
        let host = HostContext::local(cx);
        let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| Self::new(&host, content_engine, window, cx))
        })?;
        // Closing the window ends the `--low-memory` process.
        let id = handle.window_id();
        cx.on_window_closed(move |cx, closed| {
            if closed == id {
                request_exit(cx, UiExit::Quit);
            }
        })
        .detach();
        if let Some(frame) = TlSecondaryWindow::<Self>::find(handle, cx) {
            // A keyless window gets no `ToggleAlwaysOnTop`, so this window sends `KeepAbove`
            // for every pin change itself, the first one included.
            let mut sent = false;
            cx.observe(&frame, move |frame, cx| {
                let frame = frame.read(cx);
                let on = frame.pinned();
                if on != sent {
                    sent = on;
                    send_keep_above(&*engine, frame.title(), on);
                }
            })
            .detach();
            handle.update(cx, |_, window, cx| {
                frame.update(cx, |frame, cx| {
                    frame.set_pinned(true, window, cx);
                    // The title follows a language change. The words are translated once per
                    // strings version (R-132).
                    cx.observe_global_in::<Strings>(window, |frame, window, cx| {
                        let title = low_mem_words(cx).title.clone();
                        if title != *frame.title() {
                            frame.set_title(title, window, cx);
                        }
                    })
                    .detach();
                });
            })?;
        }
        Ok(handle)
    }

    /// The content of a window opened by [`Self::open`], while it is open.
    pub fn find(handle: WindowHandle<Root>, cx: &App) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    fn new(
        host: &HostContext,
        engine: Arc<dyn EngineClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        filter.update(cx, |s, cx| s.focus(window, cx));
        let list_focus = cx.focus_handle().tab_stop(true);
        let mut subscriptions = SmallVec::new();
        subscriptions.push(cx.subscribe(&filter, |this, filter, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.needle = filter.read(cx).text().to_string().to_lowercase();
                this.rebuild(&low_mem_words(cx));
                cx.notify();
            }
        }));
        let hub = host.hub().cloned();
        if let Some(hub) = &hub {
            subscriptions.push(cx.subscribe_in(
                hub,
                window,
                |this, hub, ev: &SnapshotChanged, window, cx| {
                    if !ev.mask.contains(ComponentMask::PROCESSES) {
                        return;
                    }
                    let snap = hub.read(cx).current().clone();
                    let hold = this.holds_order(window, cx);
                    if this.take_sample(&snap, hold, &low_mem_words(cx)) {
                        cx.notify();
                    }
                },
            ));
        }
        if let Some(tracker) = host.tracker().cloned() {
            subscriptions.push(
                cx.subscribe(&tracker, |this, _, ev: &ActionTrackerEvent, cx| {
                    if let ActionTrackerEvent::Finished { id, .. } = *ev {
                        let before = this.ending.len();
                        this.ending.retain(|(a, _)| *a != id);
                        if this.ending.len() != before {
                            cx.notify();
                        }
                    }
                }),
            );
        }
        // The order is held while the list has focus; it re-sorts when focus leaves and the
        // pointer is not over the list.
        subscriptions.push(cx.on_focus_out(&list_focus, window, |this, _, _, cx| {
            if !this.hovered {
                this.rebuild(&low_mem_words(cx));
                cx.notify();
            }
        }));
        // A language change rebuilds the row texts.
        subscriptions.push(cx.observe_global_in::<Strings>(window, |this, window, cx| {
            let words = low_mem_words(cx);
            if this.holds_order(window, cx) {
                this.refresh_held(&words);
            } else {
                this.rebuild(&words);
            }
            cx.notify();
        }));
        let mut this = Self {
            host: host.clone(),
            engine,
            filter,
            list_focus,
            ease: EaseTracker::default(),
            last_info: None,
            records: Arc::from([]),
            rows: Vec::new(),
            spare: Vec::new(),
            candidates: Vec::new(),
            rows_words: None,
            needle: String::new(),
            hovered: false,
            highlight: None,
            ending: SmallVec::new(),
            _subscriptions: subscriptions,
        };
        if let Some(hub) = hub {
            let snap = hub.read(cx).current().clone();
            this.take_sample(&snap, false, &low_mem_words(cx));
        }
        this
    }

    /// Whether the order is held: the pointer is over the list or focus is in it.
    fn holds_order(&self, window: &Window, cx: &App) -> bool {
        self.hovered || self.list_focus.contains_focused(window, cx)
    }

    /// Takes the sweep's pressure sample and rows. With `hold` the rows keep their order and
    /// only their values change. False when the snapshot has no new low-memory data.
    fn take_sample(&mut self, snap: &Snapshot, hold: bool, words: &Rc<LowMemWords>) -> bool {
        let Some(info) = snap.processes.low_memory.as_ref() else {
            return false;
        };
        if self
            .last_info
            .as_ref()
            .is_some_and(|last| Arc::ptr_eq(last, info))
        {
            return false;
        }
        self.ease.sample(snap.published_ns, below_threshold(info));
        self.last_info = Some(info.clone());
        self.records = snap.processes.records.clone();
        if hold {
            self.refresh_held(words);
        } else {
            self.rebuild(words);
        }
        true
    }

    /// The shown rows, and whether their texts may be reused: false when the words changed
    /// since they were built.
    fn take_prev(&mut self, words: &Rc<LowMemWords>) -> (Vec<LowMemRow>, bool) {
        let reuse = self
            .rows_words
            .as_ref()
            .is_some_and(|w| Rc::ptr_eq(w, words));
        self.rows_words = Some(words.clone());
        (std::mem::take(&mut self.rows), reuse)
    }

    /// Picks the 30 apps and processes with the most memory that match the filter, sorted by
    /// memory descending. Rows whose memory is not `Valid` sort last.
    fn rebuild(&mut self, words: &Rc<LowMemWords>) {
        let (mut prev, reuse) = self.take_prev(words);
        let mut rows = std::mem::take(&mut self.spare);
        rows.clear();
        if let Some(info) = self.last_info.clone() {
            let records = self.records.clone();
            self.collect_candidates(&info, &records);
            let uid = session_uid(&records);
            for c in &self.candidates {
                let src = match c.src {
                    (0, i) => RowSrc::App(&info.groups[i as usize]),
                    (_, i) => RowSrc::Process(&records[i as usize]),
                };
                let id = match src {
                    RowSrc::App(g) => RowId::App(g.group),
                    RowSrc::Process(r) => RowId::Process(r.key),
                };
                let old = prev.iter().find(|p| reuse && p.id == id);
                rows.push(make_row(src, info.mem_total, uid, words, old));
            }
        }
        prev.clear();
        self.spare = prev;
        self.rows = rows;
    }

    /// Fills `candidates` with the matching apps, and the matching processes outside an app
    /// group, cut at 30 and sorted. Kernel threads and tasklens itself are left out.
    fn collect_candidates(&mut self, info: &LowMemoryInfo, records: &[ProcessRecord]) {
        let needle = self.needle.as_str();
        let cands = &mut self.candidates;
        cands.clear();
        for (i, g) in info.groups.iter().enumerate() {
            if needle.is_empty() || g.name.to_lowercase().contains(needle) {
                cands.push(Candidate {
                    valid: true,
                    memory: g.memory_current,
                    src: (0, i as u32),
                });
            }
        }
        for (i, r) in records.iter().enumerate() {
            if r.flags
                .intersects(ProcFlags::KERNEL_THREAD | ProcFlags::IS_SELF)
                || info.groups.iter().any(|g| g.group == r.group)
                || !(needle.is_empty() || r.meta.search_lower.contains(needle))
            {
                continue;
            }
            let (validity, memory) = process_memory(r);
            cands.push(Candidate {
                valid: validity.is_valid(),
                memory,
                src: (1, i as u32),
            });
        }
        if cands.len() > MAX_ROWS {
            cands.select_nth_unstable_by(MAX_ROWS - 1, Candidate::order);
            cands.truncate(MAX_ROWS);
        }
        cands.sort_unstable_by(Candidate::order);
    }

    /// Updates the values of the shown rows in their current order. A row whose app or process
    /// is gone is dropped.
    fn refresh_held(&mut self, words: &Rc<LowMemWords>) {
        let (mut prev, reuse) = self.take_prev(words);
        let mut rows = std::mem::take(&mut self.spare);
        rows.clear();
        if let Some(info) = self.last_info.clone() {
            let records = self.records.clone();
            let uid = session_uid(&records);
            for old in &prev {
                let src = match old.id {
                    RowId::App(key) => info.groups.iter().find(|g| g.group == key).map(RowSrc::App),
                    RowId::Process(key) => records
                        .binary_search_by(|r| r.key.cmp(&key))
                        .ok()
                        .map(|i| RowSrc::Process(&records[i])),
                };
                if let Some(src) = src {
                    let old = reuse.then_some(old);
                    rows.push(make_row(src, info.mem_total, uid, words, old));
                }
            }
        }
        prev.clear();
        self.spare = prev;
        self.rows = rows;
    }

    pub fn filter(&self) -> &Entity<InputState> {
        &self.filter
    }

    pub fn list_focus(&self) -> &FocusHandle {
        &self.list_focus
    }

    pub fn eased(&self) -> bool {
        self.ease.eased()
    }

    /// The rows the list shows, in order.
    pub fn rows(&self) -> &[LowMemRow] {
        &self.rows
    }

    /// The names the list shows, in order.
    pub fn visible_names(&self) -> impl Iterator<Item = &SharedString> {
        self.rows.iter().map(|r| &r.name)
    }

    /// The highlighted row's index.
    pub fn highlight(&self) -> Option<usize> {
        let id = self.highlight?;
        self.rows.iter().position(|r| r.id == id)
    }

    /// The End Task tooltip of row `ix`.
    pub fn end_task_tooltip(&self, ix: usize, cx: &App) -> Option<SharedString> {
        let row = self.rows.get(ix)?;
        Some(end_task_tooltip_in(row, &words_now(cx)))
    }

    /// Whether End Task of `id` is in flight.
    pub fn is_ending(&self, id: RowId) -> bool {
        self.ending.iter().any(|(_, r)| *r == id)
    }

    /// The lines at the top: two warning lines, or the one eased line.
    pub fn header(&self, cx: &App) -> SmallVec<[HeaderLine; 2]> {
        self.header_in(&words_now(cx), current(cx))
    }

    fn header_in(&self, words: &LowMemWords, theme: &TlTheme) -> SmallVec<[HeaderLine; 2]> {
        let mut lines = SmallVec::new();
        if self.ease.eased() {
            lines.push(HeaderLine {
                text: words.eased.clone(),
                color: theme.status.success,
                strong: false,
            });
        } else {
            lines.push(HeaderLine {
                text: words.critical.clone(),
                color: theme.status.danger,
                strong: true,
            });
            lines.push(HeaderLine {
                text: words.recommend.clone(),
                color: theme.text_secondary,
                strong: false,
            });
        }
        lines
    }

    /// The text the list area shows instead of rows: only when a filter matches nothing.
    pub fn empty_text(&self, cx: &App) -> Option<SharedString> {
        self.empty_text_in(&words_now(cx))
    }

    fn empty_text_in(&self, words: &LowMemWords) -> Option<SharedString> {
        (!self.needle.is_empty() && self.rows.is_empty()).then(|| words.filter_empty.clone())
    }

    fn open_full(&mut self, _: &OpenFullTasklens, window: &mut Window, cx: &mut Context<Self>) {
        self.send_open_full(window, cx);
    }

    /// Launches the full tasklens on this window's host. While the host is reconnecting nothing
    /// is sent and this window's toast says why (ADR-018 Reconnect).
    fn send_open_full(&self, window: &mut Window, cx: &mut App) {
        if let Some(text) = reconnecting_refusal(&self.host, cx) {
            if let Some(queue) = ToastQueue::of_window(window, cx) {
                queue.update(cx, |q, cx| {
                    q.push(toast::ToastKind::Warning, text, None, None, cx)
                });
            }
            return;
        }
        // Ids come from the tracker when there is one, so an End Task id is never reused.
        let id = self
            .host
            .tracker()
            .map_or_else(next_action_id, |t| t.read(cx).next_id());
        let request = ActionRequest {
            id,
            kind: ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command(MAIN_PROGRAM.into()),
                as_admin: false,
                cwd: None,
            }),
            targets: SmallVec::new(),
            privileged: false,
        };
        if let Err(err) = self.engine.send(Command::Action(request)) {
            warn!(target: "tasklens::ui", ?err, "Open Full tasklens not sent");
        }
    }

    /// Ends the row `id` at once through the app's `ActionTracker`, with no confirmation
    /// (PROC-098). Another user's row, or one already ending, does nothing. The request carries
    /// the target captured from the sweep (PROC-001).
    fn end_task(&mut self, id: RowId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.iter().find(|r| r.id == id) else {
            return;
        };
        if row.other_user || self.is_ending(id) {
            return;
        }
        let Some(tracker) = self.host.tracker().cloned() else {
            warn!(target: "tasklens::ui", "no action tracker; End Task was not sent");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            warn!(target: "tasklens::ui", "no toast queue; End Task was not sent");
            return;
        };
        let request = end_task_request(&row.target);
        let words = low_mem_words(cx);
        // No success toast: the row leaving the list is the feedback, and extra toasts under
        // memory pressure are noise.
        let texts = ToastTexts {
            retry_as_admin: true,
            ..ToastTexts::new(SharedString::default(), words.end_failed.clone())
        };
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if let Some(action) = sent {
            self.ending.push((action, id));
            cx.notify();
        }
    }

    /// End Task on the highlighted row.
    fn end_highlighted(&mut self, _: &LowMemEndTask, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.highlight {
            self.end_task(id, window, cx);
        }
    }

    /// Moves the highlight one row up (`-1`) or down (`1`). With no highlight it starts at the
    /// first row.
    fn move_highlight(&mut self, step: isize) {
        let last = self.rows.len().checked_sub(1);
        let Some(last) = last else {
            self.highlight = None;
            return;
        };
        let at = match self.highlight() {
            Some(at) => at.saturating_add_signed(step).min(last),
            None => 0,
        };
        self.highlight = Some(self.rows[at].id);
    }

    /// One row: the name, "Memory 12.0%" or the validity glyph, and End Task.
    fn render_row(
        &self,
        i: usize,
        row: &LowMemRow,
        words: &LowMemWords,
        theme: &TlTheme,
        m: &UiMetrics,
        this: WeakEntity<Self>,
    ) -> AnyElement {
        let s = m.ui_scale;
        let selected = self.highlight == Some(row.id);
        let (primary, secondary) = if selected {
            (theme.text_selected_primary, theme.text_selected_secondary)
        } else {
            (theme.text_primary, theme.text_secondary)
        };

        let mut name = div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(primary);
        *name.text_style() = t_base(m);

        let memory = if row.validity.is_valid() {
            let mut text = div()
                .debug_selector(move || format!("LowMemMemory-{i}"))
                .flex()
                .justify_end()
                .whitespace_nowrap()
                .text_color(secondary);
            *text.text_style() = numeric_style(m, m.text.sm);
            text.child(row.memory_text.clone()).into_any_element()
        } else {
            glyph(row.validity, &row.memory_text, selected, m, theme)
        };

        let id = row.id;
        let tooltip = end_task_tooltip_in(row, words);
        let button = TlButton::new(("LowMemEndTask", i), TlButtonKind::Danger)
            .size(TlButtonSize::Sm)
            .label(words.end_task.clone())
            .accessible_name(row.a11y.clone())
            .tooltip(tooltip)
            .disabled(row.other_user)
            .loading(self.is_ending(id))
            // The list is the one focus stop; the buttons are reached with the highlight.
            .tab_stop(false)
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.end_task(id, window, cx));
            });

        div()
            .id(("LowMemRow", i))
            .debug_selector(move || format!("LowMemRow-{i}"))
            .flex_none()
            .flex()
            .items_center()
            .gap(m.sp[4])
            .h(px(ROW * s))
            .px(m.sp[4])
            .border_b_1()
            .border_color(theme.divider)
            .when(selected, |r| r.bg(theme.row_selected))
            .child(name.child(row.name.clone()))
            .child(div().flex_none().w(px(MEMORY_W * s)).child(memory))
            .child(
                div()
                    .debug_selector(move || format!("LowMemEndTask-{i}"))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .w(px(BUTTON_W * s))
                    .child(button),
            )
            .into_any_element()
    }

    fn clear_or_close(
        &mut self,
        _: &LowMemClearOrClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.filter.read(cx).text().len() > 0 {
            self.filter.update(cx, |s, cx| s.clean(window, cx));
        } else {
            window.remove_window();
        }
    }

    fn close(&mut self, _: &LowMemClose, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    /// Up and Down move the highlight; Enter and Delete end the highlighted row. A printable
    /// character typed in the list moves focus to the filter with that character.
    fn list_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let mods = ks.modifiers;
        if mods.control || mods.alt || mods.platform || mods.function {
            return;
        }
        if !mods.shift {
            match ks.key.as_str() {
                "up" | "down" => {
                    self.move_highlight(if ks.key == "up" { -1 } else { 1 });
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                "enter" | "delete" => {
                    self.end_highlighted(&LowMemEndTask, window, cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        let Some(text) = ks.key_char.as_deref() else {
            return;
        };
        if text.is_empty() || text.chars().any(char::is_control) {
            return;
        }
        TlFilterBox::focus_and_insert(&self.filter, text, window, cx);
        cx.stop_propagation();
    }
}

impl Render for LowMemoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let words = low_mem_words(cx);
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;

        let header = self.header_in(&words, &theme).into_iter().map(|line| {
            let mut text = div().debug_selector(|| "LowMemHeaderLine".into());
            *text.text_style() = if line.strong {
                label_style(&m, m.text.md, FontWeight::SEMIBOLD)
            } else {
                t_base(&m)
            };
            text.text_color(line.color).child(line.text)
        });

        let list_focused = self.list_focus.is_focused(window);
        let mut list = div()
            .id("LowMemList")
            .debug_selector(|| "LowMemList".into())
            .track_focus(&self.list_focus)
            .on_key_down(cx.listener(Self::list_key_down))
            .on_hover(cx.listener(|this, hovered: &bool, window, cx| {
                this.hovered = *hovered;
                if !*hovered && !this.list_focus.contains_focused(window, cx) {
                    this.rebuild(&low_mem_words(cx));
                    cx.notify();
                }
            }))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .border_1()
            .border_color(if list_focused {
                theme.accent
            } else {
                theme.divider
            });
        match self.empty_text_in(&words) {
            Some(text) => {
                let mut empty = div()
                    .debug_selector(|| "LowMemFilterEmpty".into())
                    .p(m.sp[4])
                    .text_color(theme.text_secondary);
                *empty.text_style() = t_base(&m);
                list = list.child(empty.child(text));
            }
            None => {
                let this = cx.entity().downgrade();
                list = list.children(
                    self.rows
                        .iter()
                        .enumerate()
                        .map(|(i, row)| self.render_row(i, row, &words, &theme, &m, this.clone())),
                );
            }
        }

        let this = cx.entity().downgrade();
        let open_full = TlButton::new("LowMemOpenFull", TlButtonKind::Primary)
            .label(words.open_full.clone())
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.send_open_full(window, cx));
            });

        div()
            .id("LowMemory")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::clear_or_close))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::open_full))
            .on_action(cx.listener(Self::end_highlighted))
            .size_full()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .p(m.sp[6])
            .bg(theme.window_bg)
            .child(div().flex().flex_col().gap(m.sp[2]).children(header))
            .child(
                TlTextInput::new("LowMemFilter", &self.filter)
                    .size(TlInputSize::Md)
                    .clear_button(true)
                    .clean_on_escape(true)
                    .placeholder(words.filter_placeholder.clone()),
            )
            .child(list)
            .child(
                div()
                    .debug_selector(|| "LowMemFooter".into())
                    .flex_none()
                    .flex()
                    .items_center()
                    .h(px(FOOTER * s))
                    .child(
                        div()
                            .debug_selector(|| "LowMemOpenFull".into())
                            .child(open_full),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(mem_available: u64, psi: Option<f32>) -> LowMemoryInfo {
        LowMemoryInfo {
            groups: Arc::from([]),
            mem_total: 1000,
            mem_available,
            psi_full_pct: psi,
            critical: false,
        }
    }

    #[test]
    fn threshold_edges() {
        assert!(below_threshold(&info(30, Some(10.))));
        assert!(below_threshold(&info(30, None)));
        assert!(!below_threshold(&info(29, Some(0.))));
        assert!(!below_threshold(&info(500, Some(10.5))));
    }

    #[test]
    fn ease_needs_10s_of_calm() {
        let mut ease = EaseTracker::default();
        let s = 1_000_000_000;
        ease.sample(0, false);
        for t in [2, 4, 6, 8, 10] {
            ease.sample(t * s, true);
            assert!(!ease.eased(), "{t}");
        }
        ease.sample(12 * s, true);
        assert!(ease.eased());
        ease.sample(14 * s, false);
        assert!(!ease.eased());
        ease.sample(16 * s, true);
        assert!(!ease.eased());
    }
}
