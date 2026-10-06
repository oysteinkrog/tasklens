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
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::base::Root;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, AsyncApp, Context, Entity, FocusHandle, FontWeight, Global, Hsla,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Subscription, Window, WindowHandle, div, px, size,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, LaunchSpec, LaunchWhat};
use tl_model::client::EngineClient;
use tl_model::command::{Command, KWinCommand};
use tl_model::process::LowMemoryInfo;
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::time::TimeNs;
use tracing::{debug, warn};

use crate::app::{
    AppRequestHandler, BootSettings, EngineGlobal, OpenHooks, UiExit, UiInit, request_exit,
};
use crate::components::button::{TlButton, TlButtonKind};
use crate::components::input::{TlFilterBox, TlInputSize, TlTextInput};
use crate::components::toast;
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{self, Strings, tr, tr_en};
use crate::settings_global::TlSettings;
use crate::text::style::{label_style, t_base};
use crate::theme::user_store::ThemePersist;
use crate::theme::{self, TlTheme, current, metrics};

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
    cx.set_global(GlobalHub(hub));

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

    cx.default_global::<OpenHooks>().low_memory = Some(Box::new(move |cx| {
        if let Err(err) = LowMemoryWindow::open(cx, client.clone()) {
            warn!(target: "tasklens::ui", %err, "rescue window did not open");
            request_exit(cx, UiExit::Quit);
        }
    }));
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
}

impl LowMemWords {
    fn with(t: impl Fn(&str) -> SharedString) -> Self {
        Self {
            title: t(KEY_TITLE),
            critical: t(KEY_CRITICAL),
            recommend: t(KEY_RECOMMEND),
            eased: t(KEY_EASED),
            filter_placeholder: t(KEY_FILTER_PLACEHOLDER),
            filter_empty: t(KEY_FILTER_EMPTY),
            open_full: t(KEY_OPEN_FULL),
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
        LowMemWords::with(|key| tr(cx, key))
    } else {
        LowMemWords::with(|key| tr_en(key, &[]))
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
    engine: Arc<dyn EngineClient>,
    filter: Entity<InputState>,
    /// The list is one focus stop.
    list_focus: FocusHandle,
    ease: EaseTracker,
    /// The last sample taken, compared by pointer so a sweep counts once.
    last_info: Option<Arc<LowMemoryInfo>>,
    /// Row names (E32-A2 adds the processes and the values).
    names: Vec<SharedString>,
    names_lower: Vec<String>,
    /// Indexes into `names` that match the filter.
    visible: Vec<u32>,
    /// The lowercased filter text.
    needle: String,
    _subscriptions: SmallVec<[Subscription; 2]>,
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
        let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| Self::new(content_engine, window, cx))
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

    fn new(engine: Arc<dyn EngineClient>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx));
        filter.update(cx, |s, cx| s.focus(window, cx));
        let mut subscriptions = SmallVec::new();
        subscriptions.push(cx.subscribe(&filter, |this, filter, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.needle = filter.read(cx).text().to_string().to_lowercase();
                this.refilter();
                cx.notify();
            }
        }));
        let hub = cx.try_global::<GlobalHub>().map(|hub| hub.0.clone());
        if let Some(hub) = &hub {
            subscriptions.push(cx.subscribe(hub, |this, hub, ev: &SnapshotChanged, cx| {
                if ev.mask.contains(ComponentMask::PROCESSES)
                    && this.take_sample(&hub.read(cx).current().clone())
                {
                    cx.notify();
                }
            }));
        }
        let mut this = Self {
            engine,
            filter,
            list_focus: cx.focus_handle().tab_stop(true),
            ease: EaseTracker::default(),
            last_info: None,
            names: Vec::new(),
            names_lower: Vec::new(),
            visible: Vec::new(),
            needle: String::new(),
            _subscriptions: subscriptions,
        };
        if let Some(hub) = hub {
            this.take_sample(&hub.read(cx).current().clone());
        }
        this
    }

    /// Takes the sweep's pressure sample and row names. False when the snapshot has no new
    /// low-memory data.
    fn take_sample(&mut self, snap: &Snapshot) -> bool {
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
        // E32-A2: the processes, the values, the order and the 30-row cut.
        self.names = info
            .groups
            .iter()
            .map(|g| SharedString::from(g.name.to_string()))
            .collect();
        self.names_lower = self.names.iter().map(|n| n.to_lowercase()).collect();
        self.last_info = Some(info.clone());
        self.refilter();
        true
    }

    fn refilter(&mut self) {
        self.visible.clear();
        self.visible.extend(
            self.names_lower
                .iter()
                .enumerate()
                .filter(|(_, name)| name.contains(self.needle.as_str()))
                .map(|(i, _)| i as u32),
        );
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

    /// The names the list shows, in order.
    pub fn visible_names(&self) -> impl Iterator<Item = &SharedString> {
        self.visible.iter().map(|&i| &self.names[i as usize])
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
        (!self.needle.is_empty() && self.visible.is_empty()).then(|| words.filter_empty.clone())
    }

    fn open_full(&mut self, _: &OpenFullTasklens, _: &mut Window, _: &mut Context<Self>) {
        self.send_open_full();
    }

    fn send_open_full(&self) {
        let request = ActionRequest {
            id: next_action_id(),
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

    /// A printable character typed in the list moves focus to the filter with that character.
    fn list_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let mods = ks.modifiers;
        if mods.control || mods.alt || mods.platform || mods.function {
            return;
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
                // E32-A2: the full rows (memory share and End Task).
                list = list.children(self.visible.iter().map(|&i| {
                    let mut row = div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .h(px(ROW * s))
                        .px(m.sp[4])
                        .border_b_1()
                        .border_color(theme.divider)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(theme.text_primary);
                    *row.text_style() = t_base(&m);
                    row.child(self.names[i as usize].clone())
                }));
            }
        }

        let this = cx.entity().downgrade();
        let open_full = TlButton::new("LowMemOpenFull", TlButtonKind::Primary)
            .label(words.open_full.clone())
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, _| this.send_open_full());
            });

        div()
            .id("LowMemory")
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::clear_or_close))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::open_full))
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
