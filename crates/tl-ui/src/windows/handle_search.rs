//! The Search File Handles window "tasklens Handle Search": pattern, include-maps option,
//! Search and Stop, progress, the footer note and "Search all" through the helper
//! (UI-PWN-030, UI-SHL-098, PROC-096, APP-015, R-061, R-102, R-119). The result table and its
//! menu are E23-031 and fill the table slot.
//!
//! Security: "Search all" only dispatches `StartHelperSession`; the engine elevates. The
//! pattern and the paths are never logged (ADR-015).
//!
//! The words are `en-US.ftl` keys, translated once per strings version into
//! [`HandleSearchWords`].

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::input::{Enter, Escape, InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Command;
use tl_model::details::{HandlePhase, HandleSearchState, SearchId};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::snapshot::{ComponentMask, Snapshot};
use tl_model::units;
use tracing::warn;

use super::actions::{
    ClearHandleSearch, HANDLE_SEARCH_CONTEXT, HandleSearchAll, StartHandleSearch, StopHandleSearch,
};
use super::frame::{PwnWindow, TITLE_HANDLE_SEARCH, pwn_window_spec};
use crate::actions::OpenHandleSearch;
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::checkbox::TlCheckbox;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::progress::{TlProgress, TlProgressSize};
use crate::components::shield::StartHelperSession;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::components::window_frame::{CloseSecondaryWindow, TlSecondaryWindow};
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::text::style::{numeric_style, t_base, t_sm};
use crate::theme::{current, metrics};
use crate::window::registry::{GlobalRegistry, WindowKey};

/// Pattern row, checkbox row, progress row and footer heights, × ui_scale.
const PATTERN_ROW_H: f32 = 40.;
const CHECK_ROW_H: f32 = 28.;
const PROGRESS_ROW_H: f32 = 24.;
const FOOTER_H: f32 = 44.;
/// Progress bar width, × ui_scale.
const PROGRESS_W: f32 = 200.;

/// The next search id, process-global so two windows never share one.
static NEXT_SEARCH: AtomicU64 = AtomicU64::new(1);

/// Where the window is in its search life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchStatus {
    Idle,
    Searching,
    Done,
    Stopped,
    Failed,
}

/// The window's words, translated once per strings version.
pub struct HandleSearchWords {
    strings_version: Option<u64>,
    pub pattern: SharedString,
    pub pattern_tooltip: SharedString,
    pub include_maps: SharedString,
    pub search: SharedString,
    pub stop: SharedString,
    pub clear: SharedString,
    pub search_all: SharedString,
    pub close: SharedString,
    pub empty: SharedString,
    pub initializing: SharedString,
    pub searching: SharedString,
    pub none: SharedString,
    pub failed: SharedString,
    pub toast_failed: SharedString,
    pub toast_auth_cancelled: SharedString,
    retrieving: Template,
    found: Template,
    stopped: Template,
    footer: Template,
}

impl HandleSearchWords {
    fn new(cx: &App) -> Self {
        Self {
            strings_version: strings_version(cx),
            pattern: tr(cx, "pwn-handle-search-pattern"),
            pattern_tooltip: tr(cx, "pwn-handle-search-pattern-tooltip"),
            include_maps: tr(cx, "pwn-handle-search-include-maps"),
            search: tr(cx, "pwn-handle-search-search"),
            stop: tr(cx, "pwn-handle-search-stop"),
            clear: tr(cx, "pwn-handle-search-clear"),
            search_all: tr(cx, "pwn-handle-search-search-all"),
            close: tr(cx, "pwn-handle-search-close"),
            empty: tr(cx, "pwn-handle-search-empty"),
            initializing: tr(cx, "pwn-handle-search-initializing"),
            searching: tr(cx, "pwn-handle-search-searching"),
            none: tr(cx, "pwn-handle-search-none"),
            failed: tr(cx, "pwn-handle-search-failed"),
            toast_failed: tr(cx, "pwn-handle-search-toast-failed"),
            toast_auth_cancelled: tr(cx, "pwn-handle-search-toast-auth-cancelled"),
            retrieving: Template::new(cx, "pwn-handle-search-retrieving", &["pct", "n"]),
            found: Template::counted(cx, "pwn-handle-search-found", &["n"], 0),
            stopped: Template::counted(cx, "pwn-handle-search-stopped", &["n"], 0),
            footer: Template::new(
                cx,
                "pwn-handle-search-footer",
                &["searched", "total", "skipped"],
            ),
        }
    }

    /// "Retrieving file handles... 42% (18 234 total handles)".
    pub fn retrieving_text(&self, pct: u8, total_handles: u64) -> SharedString {
        let mut n = String::new();
        units::count(&mut n, total_handles);
        let mut out = String::new();
        let _ = self.retrieving.write(&mut out, &[&pct.min(100), &n]);
        out.into()
    }

    /// "Found 12 matches", "Found 1 match" or "No results found.".
    pub fn found_text(&self, hits: usize) -> SharedString {
        if hits == 0 {
            return self.none.clone();
        }
        let mut n = String::new();
        units::count(&mut n, hits as u64);
        let mut out = String::new();
        let _ = self.found.write_count(&mut out, hits as u64, &[&n]);
        out.into()
    }

    /// "Search stopped. Found 12 matches".
    pub fn stopped_text(&self, hits: usize) -> SharedString {
        let mut n = String::new();
        units::count(&mut n, hits as u64);
        let mut out = String::new();
        let _ = self.stopped.write_count(&mut out, hits as u64, &[&n]);
        out.into()
    }

    /// "Searched 412 of 530 processes. 118 need administrator rights.".
    pub fn footer_text(&self, searched: u32, total: u32, skipped: u32) -> SharedString {
        let count = |v: u32| {
            let mut s = String::new();
            units::count(&mut s, u64::from(v));
            s
        };
        let (a, b, c) = (count(searched), count(total), count(skipped));
        let mut out = String::new();
        let _ = self.footer.write(&mut out, &[&a, &b, &c]);
        out.into()
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

fn send(cmd: Command, cx: &App) {
    match cx.try_global::<EngineGlobal>() {
        Some(engine) => {
            if let Err(err) = engine.0.send(cmd) {
                warn!(target: "tasklens::ui", ?err, "handle search command not sent");
            }
        }
        None => warn!(target: "tasklens::ui", "no engine, handle search command not sent"),
    }
}

/// Registers the `OpenHandleSearch` handler (File > Search File Handles...).
/// `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu the main window is mid-update.
    cx.on_action(|_: &OpenHandleSearch, cx| cx.defer(open_handle_search));
}

/// Activates the open Handle Search window, or opens one. Without a window registry (tests) it
/// always opens one.
pub fn open_handle_search(cx: &mut App) {
    let open = |cx: &mut App| {
        let spec = pwn_window_spec(
            cx,
            PwnWindow::HandleSearch,
            TITLE_HANDLE_SEARCH.into(),
            None,
        )
        .key(WindowKey::HandleSearch);
        let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| HandleSearchWindow::new(window, cx))
        })?;
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| {
                r.open_or_focus(WindowKey::HandleSearch, cx, open)
            });
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// The content of the Handle Search window.
pub struct HandleSearchWindow {
    pattern: Entity<InputState>,
    include_maps: bool,
    /// The current search; payloads of any other id are dropped.
    search: Option<SearchId>,
    state: SearchStatus,
    last: Option<Arc<HandleSearchState>>,
    hits: usize,
    /// "Search all" waits for the helper session.
    pending_all: bool,
    helper: HelperState,
    start_seq: Option<u64>,
    failed_toast: bool,
    words: Rc<HandleSearchWords>,
    focus: FocusHandle,
    check_focus: FocusHandle,
    _subs: SmallVec<[Subscription; 3]>,
}

impl HandleSearchWindow {
    /// The content of the Handle Search window `handle`, while it is open.
    pub fn find(
        handle: gpui_kit::WindowHandle<gpui_kit::base::Root>,
        cx: &App,
    ) -> Option<Entity<Self>> {
        TlSecondaryWindow::<Self>::find(handle, cx).map(|frame| frame.read(cx).content().clone())
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let words = Rc::new(HandleSearchWords::new(cx));
        let pattern = cx.new(|cx| InputState::new(window, cx));
        pattern.update(cx, |s, cx| s.focus(window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&pattern, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));
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
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            if this.words.strings_version != strings_version(cx) {
                this.words = Rc::new(HandleSearchWords::new(cx));
                cx.notify();
            }
        }));
        // Closing the window while searching stops the search.
        cx.on_release(|this, cx| {
            if let (SearchStatus::Searching, Some(id)) = (this.state, this.search) {
                send(Command::CancelHandleSearch(id), cx);
            }
        })
        .detach();
        let mut this = Self {
            pattern,
            include_maps: true,
            search: None,
            state: SearchStatus::Idle,
            last: None,
            hits: 0,
            pending_all: false,
            helper: HelperState::default(),
            start_seq: None,
            failed_toast: false,
            words,
            focus: cx.focus_handle(),
            check_focus: cx.focus_handle(),
            _subs: subs,
        };
        if let Some(hub) = hub {
            let snap = hub.read(cx).current().clone();
            // An outcome from before the window opened never toasts here.
            this.helper = snap.health.helper;
            this.start_seq = snap.health.helper_start.as_ref().map(|(seq, _)| *seq);
        }
        this
    }

    // ---- Accessors ----

    pub fn pattern(&self) -> &Entity<InputState> {
        &self.pattern
    }

    pub fn search_id(&self) -> Option<SearchId> {
        self.search
    }

    pub fn state(&self) -> SearchStatus {
        self.state
    }

    /// The last payload of the current search.
    pub fn last_state(&self) -> Option<&Arc<HandleSearchState>> {
        self.last.as_ref()
    }

    /// Hits so far: the sum of the chunk lengths.
    pub fn hit_count(&self) -> usize {
        self.hits
    }

    pub fn include_maps(&self) -> bool {
        self.include_maps
    }

    pub fn is_searching(&self) -> bool {
        self.state == SearchStatus::Searching
    }

    /// "Search all" waits for the helper session.
    pub fn is_pending_all(&self) -> bool {
        self.pending_all
    }

    pub fn words(&self) -> &Rc<HandleSearchWords> {
        &self.words
    }

    fn pattern_text(&self, cx: &App) -> String {
        self.pattern.read(cx).text().trim().to_owned()
    }

    /// Search is enabled: a pattern and no search running. While searching the button is Stop.
    pub fn search_enabled(&self, cx: &App) -> bool {
        self.is_searching() || !self.pattern_text(cx).is_empty()
    }

    /// The status text, empty before the first search.
    pub fn status_text(&self, _cx: &App) -> SharedString {
        let w = &self.words;
        match self.state {
            SearchStatus::Idle => SharedString::default(),
            SearchStatus::Searching => match self.last.as_deref() {
                Some(s) => match s.phase {
                    HandlePhase::Retrieving => w.retrieving_text(s.progress_pct, s.total_handles),
                    HandlePhase::Searching => w.searching.clone(),
                    _ => w.initializing.clone(),
                },
                None => w.initializing.clone(),
            },
            SearchStatus::Done => w.found_text(self.hits),
            SearchStatus::Stopped => w.stopped_text(self.hits),
            SearchStatus::Failed => w.failed.clone(),
        }
    }

    /// The privilege note, only when processes were skipped.
    pub fn footer_note(&self) -> Option<SharedString> {
        let s = self.last.as_deref()?;
        (s.skipped > 0).then(|| self.words.footer_text(s.searched, s.total, s.skipped))
    }

    /// "Search all" shows when processes were skipped.
    pub fn search_all_shown(&self) -> bool {
        self.last.as_deref().is_some_and(|s| s.skipped > 0)
    }

    // ---- Searching ----

    /// Sends a new search for the pattern. Ignored for an empty pattern or while one runs.
    pub fn start(&mut self, privileged: bool, cx: &mut Context<Self>) {
        let text = self.pattern_text(cx);
        if text.is_empty() || self.is_searching() {
            return;
        }
        let id = SearchId(NEXT_SEARCH.fetch_add(1, Ordering::Relaxed));
        self.search = Some(id);
        self.last = None;
        self.hits = 0;
        self.failed_toast = false;
        self.state = SearchStatus::Searching;
        send(
            Command::HandleSearch {
                search: id,
                pattern: text.as_str().into(),
                include_maps: self.include_maps,
                privileged,
            },
            cx,
        );
        cx.notify();
    }

    /// Stops a running search and keeps its results.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let (SearchStatus::Searching, Some(id)) = (self.state, self.search) {
            send(Command::CancelHandleSearch(id), cx);
            self.state = SearchStatus::Stopped;
            cx.notify();
        }
    }

    /// Stops a running search, and empties the pattern and the results.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop(cx);
        self.pattern.update(cx, |s, cx| s.set_value("", window, cx));
        self.last = None;
        self.hits = 0;
        self.search = None;
        self.pending_all = false;
        self.state = SearchStatus::Idle;
        cx.notify();
    }

    /// "Search all": at once when the helper session is active, else after it starts.
    pub fn search_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_searching() || self.pending_all || self.pattern_text(cx).is_empty() {
            return;
        }
        if self.helper == HelperState::Unlocked {
            self.start(true, cx);
        } else {
            self.pending_all = true;
            window.dispatch_action(Box::new(StartHelperSession), cx);
            cx.notify();
        }
    }

    fn take_snapshot(
        &mut self,
        snap: &Snapshot,
        mask: ComponentMask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mask.intersects(ComponentMask::HEALTH) {
            self.take_health(snap, window, cx);
        }
        if mask.intersects(ComponentMask::DETAILS) {
            self.take_search(snap.details.handle_search.as_ref(), window, cx);
        }
    }

    fn take_health(&mut self, snap: &Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        let health = &snap.health;
        if let Some((seq, outcome)) = &health.helper_start
            && self.start_seq != Some(*seq)
        {
            self.start_seq = Some(*seq);
            if self.pending_all && *outcome == HelperStartOutcome::Cancelled {
                self.pending_all = false;
                let text = self.words.toast_auth_cancelled.clone();
                toast(window, cx, ToastKind::Info, text);
                cx.notify();
            }
        }
        let was = self.helper;
        self.helper = health.helper;
        if self.helper == HelperState::Unlocked && was != HelperState::Unlocked && self.pending_all
        {
            self.pending_all = false;
            self.start(true, cx);
        }
        if was != self.helper {
            cx.notify();
        }
    }

    fn take_search(
        &mut self,
        payload: Option<&Arc<HandleSearchState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(payload) = payload else {
            return;
        };
        if Some(payload.search) != self.search {
            return;
        }
        if self.last.as_ref().is_some_and(|l| Arc::ptr_eq(l, payload)) {
            return;
        }
        self.hits = payload.chunks.iter().map(|c| c.len()).sum();
        self.last = Some(payload.clone());
        // A stopped search keeps its text; later payloads only add results.
        if self.state == SearchStatus::Searching {
            match payload.phase {
                HandlePhase::Done => self.state = SearchStatus::Done,
                HandlePhase::Stopped => self.state = SearchStatus::Stopped,
                HandlePhase::Failed => {
                    self.state = SearchStatus::Failed;
                    if !std::mem::replace(&mut self.failed_toast, true) {
                        let text = self.words.toast_failed.clone();
                        toast(window, cx, ToastKind::Error, text);
                    }
                }
                _ => {}
            }
        }
        cx.notify();
    }

    // ---- Actions ----

    fn on_start(&mut self, _: &StartHandleSearch, _: &mut Window, cx: &mut Context<Self>) {
        self.start(false, cx);
    }

    /// Stop while searching, else close the window.
    fn on_stop(&mut self, _: &StopHandleSearch, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_searching() {
            self.stop(cx);
        } else {
            window.dispatch_action(Box::new(CloseSecondaryWindow), cx);
        }
    }

    fn on_clear(&mut self, _: &ClearHandleSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.clear(window, cx);
    }

    fn on_search_all(&mut self, _: &HandleSearchAll, window: &mut Window, cx: &mut Context<Self>) {
        self.search_all(window, cx);
    }

    /// Esc while searching stops; otherwise the frame closes the window.
    fn on_close(&mut self, _: &CloseSecondaryWindow, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_searching() {
            self.stop(cx);
        } else {
            cx.propagate();
        }
    }
}

/// A toast in the window's own stack.
fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack in the Handle Search window"),
    }
}

impl Focusable for HandleSearchWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for HandleSearchWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        let words = self.words.clone();
        let searching = self.is_searching();
        let view = cx.entity();

        let mut label = div().flex_none().text_color(theme.text_primary);
        *label.text_style() = t_base(&m);
        let input = div()
            .id("HandleSearchPatternTip")
            .flex_1()
            .min_w_0()
            .tooltip(TlTooltip::text(words.pattern_tooltip.clone()).build())
            .child(
                TlTextInput::new("HandleSearchPattern", &self.pattern)
                    .size(TlInputSize::Md)
                    .aria_label(words.pattern.trim_end_matches(':').to_owned())
                    .disabled(searching),
            );
        let search_button = if searching {
            TlButton::new("HandleSearchStop", TlButtonKind::Secondary)
                .label(words.stop.clone())
                .action(Box::new(StopHandleSearch))
        } else {
            TlButton::new("HandleSearchStart", TlButtonKind::Primary)
                .label(words.search.clone())
                .disabled(!self.search_enabled(cx))
                .action(Box::new(StartHandleSearch))
        };
        let clear_button = TlButton::new("HandleSearchClear", TlButtonKind::Secondary)
            .label(words.clear.clone())
            .action(Box::new(ClearHandleSearch));
        let pattern_row = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .h(px(PATTERN_ROW_H * s))
            .child(label.child(words.pattern.clone()))
            .child(input)
            .child(search_button)
            .child(clear_button);

        let check = {
            let view = view.clone();
            TlCheckbox::new("HandleSearchIncludeMaps")
                .focus_handle(self.check_focus.clone())
                .checked(Some(self.include_maps))
                .label(words.include_maps.clone())
                .disabled(searching)
                .on_change(move |on, _, cx| {
                    view.update(cx, |this, cx| {
                        this.include_maps = on;
                        cx.notify();
                    });
                })
        };
        let check_row = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(CHECK_ROW_H * s))
            .child(check);

        let progress_row = (self.state != SearchStatus::Idle).then(|| {
            let mut text = div()
                .debug_selector(|| "HandleSearchStatus".into())
                .text_color(theme.text_secondary)
                .whitespace_nowrap();
            *text.text_style() = numeric_style(&m, m.text.sm);
            let pct = self
                .last
                .as_deref()
                .filter(|l| l.phase == HandlePhase::Retrieving)
                .map(|l| f32::from(l.progress_pct));
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(m.sp[4])
                .h(px(PROGRESS_ROW_H * s))
                .when(searching, |d| {
                    d.child(
                        div()
                            .debug_selector(|| "HandleSearchProgress".into())
                            .flex_none()
                            .w(px(PROGRESS_W * s))
                            .child(
                                TlProgress::new()
                                    .value(pct)
                                    .size(TlProgressSize::Md)
                                    .reduced_motion(true),
                            ),
                    )
                })
                .child(text.child(self.status_text(cx)))
        });

        // The slot of the result table (E23-031).
        let mut empty = div()
            .debug_selector(|| "HandleSearchEmpty".into())
            .text_color(theme.text_secondary);
        *empty.text_style() = t_base(&m);
        let table = div()
            .debug_selector(|| "HandleSearchTable".into())
            .flex_1()
            .min_h_0()
            .when(self.state == SearchStatus::Idle, |d| {
                d.child(empty.child(words.empty.clone()))
            });

        let note = self.footer_note().map(|text| {
            let mut note = div()
                .debug_selector(|| "HandleSearchNote".into())
                .text_color(theme.text_secondary);
            *note.text_style() = t_sm(&m);
            note.child(text)
        });
        let search_all = self.search_all_shown().then(|| {
            TlButton::new("HandleSearchAll", TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .label(words.search_all.clone())
                .loading(self.pending_all)
                .disabled(searching || self.pending_all)
                .action(Box::new(HandleSearchAll))
        });
        let close = TlButton::new("HandleSearchClose", TlButtonKind::Secondary)
            .label(words.close.clone())
            .on_click(|_, window, _| window.remove_window());
        let footer = div()
            .debug_selector(|| "HandleSearchFooter".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(m.sp[4])
            .h(px(FOOTER_H * s))
            .px(m.sp[5])
            .border_t_1()
            .border_color(theme.divider)
            .children(note)
            .children(search_all)
            .child(div().flex_1())
            .child(close);

        let (enter, escape) = (view.clone(), view);
        div()
            .id("HandleSearchWindow")
            .key_context(HANDLE_SEARCH_CONTEXT)
            .track_focus(&self.focus)
            // The input binds Enter and Esc itself, so the window takes them first.
            .capture_action(move |_: &Enter, _, cx| {
                cx.stop_propagation();
                enter.update(cx, |this, cx| this.start(false, cx));
            })
            .capture_action(move |_: &Escape, _, cx| {
                if escape.read(cx).is_searching() {
                    cx.stop_propagation();
                    escape.update(cx, |this, cx| this.stop(cx));
                }
            })
            .on_action(cx.listener(Self::on_start))
            .on_action(cx.listener(Self::on_stop))
            .on_action(cx.listener(Self::on_clear))
            .on_action(cx.listener(Self::on_search_all))
            .on_action(cx.listener(Self::on_close))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(m.sp[3])
                    .p(m.sp[5])
                    .child(pattern_row)
                    .child(check_row)
                    .children(progress_row)
                    .child(table),
            )
            .child(footer)
    }
}
