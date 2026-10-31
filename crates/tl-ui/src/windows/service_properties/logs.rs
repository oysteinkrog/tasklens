//! Service Properties Logs tab (UI-SYS-036, SYS-015, R-074, R-118).
//!
//! The unit's journal for the current boot: a toolbar (filter, Follow, Load older, Copy, Open in
//! terminal) over a monospace list with time, priority and message columns, coloured by
//! priority. The tab opens a `JournalHandle` when it is first shown; the engine sends the last
//! 200 entries of this boot first. Each entry is formatted once on arrival.
//!
//! Privacy: journal messages are shown and copied on request only, never logged. The engine
//! sanitizes message text before it arrives (E14-A4).
//!
//! Security: Open in terminal puts the unit id into a command line that the engine runs through
//! `sh -c`. [`quote_exec_arg`] wraps it in double quotes and escapes `"`, `` ` ``, `$` and `\`.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use fluent_bundle::FluentValue;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels,
    Render, Role, ScrollStrategy, ScrollWheelEvent, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, TextStyleRefinement, UniformListScrollHandle, Window, div, px,
    uniform_list,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, LaunchSpec, LaunchWhat};
use tl_model::journal::{
    JournalAccess, JournalEntry, JournalError, JournalEvent, JournalHandle, JournalQuery,
};
use tl_model::snapshot::UnitDetails;
use tl_model::table::{Manager, UnitRef};
use tl_model::units::{self, LocalDateTime};
use tracing::warn;

use super::ServicePropsTabView;
use crate::action_tracker::{ActionTrackerEvent, ToastTexts};
use crate::actions::OpenAdditionalSetup;
use crate::components::banner::{TlBanner, TlBannerKind};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::empty::{TlEmptyState, TlEmptyTone};
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_ROW_NAME, HoverTooltip, TlTooltip};
use crate::host::HostContext;
use crate::i18n::{tr, tr_args};
use crate::pages::services::actions::{
    JOURNAL_LIST_CONTEXT, ServiceLogsBottom, ServiceLogsCopySelected, ServiceLogsSelectAll,
    ServiceLogsTop,
};
use crate::pages::sys_status::{LabelTone, tone_color};
use crate::table::selection::{Selection, SelectionView};
use crate::text::fonts::MONO_FAMILY;
use crate::theme::{TlTheme, current, metrics};
use crate::window::setup::SetupRow;

/// Quiet time after the last keystroke before the filter applies.
pub const FILTER_DEBOUNCE: Duration = Duration::from_millis(100);
/// Toolbar height, × ui_scale.
const TOOLBAR_H: f32 = 32.;
/// Filter input width, × ui_scale.
const FILTER_W: f32 = 220.;
/// List line height, × ui_scale.
const LINE_H: f32 = 18.;
/// The list keeps at most this many lines (UI-SYS-037).
pub const MAX_LINES: usize = 10_000;
/// Gap between the new-lines pill and the list bottom, × ui_scale.
const PILL_BOTTOM: f32 = 16.;
/// Noto Sans Mono advance width as a fraction of the font size (600 of 1000 units).
const MONO_ADVANCE: f32 = 0.6;
/// Width of the time column in characters ("YYYY-MM-DD HH:MM:SS").
const TIME_CHARS: f32 = 19.;
/// Width of the priority column in characters ("warning").
const PRIO_CHARS: f32 = 7.;
/// The message tooltip shows at most this many characters.
const TOOLTIP_CHARS: usize = 2000;

gpui_kit::actions!(tasklens, [ServiceLogsRetry, ServiceLogsCopyCommand]);

/// What the list area shows (UI-SYS-038).
#[derive(Clone, Debug, PartialEq, Eq)]
enum ListState {
    Error(JournalError),
    Loading,
    NoEntries,
    NoMatch,
    List,
}

/// The first and the fallback terminal launcher.
const TERMINALS: [&str; 2] = ["xdg-terminal-exec", "konsole -e"];

/// One journal entry, formatted once on arrival.
#[derive(Clone, Debug)]
pub struct LogLine {
    /// Local "YYYY-MM-DD HH:MM:SS".
    pub time: SharedString,
    pub prio_word: SharedString,
    pub priority: u8,
    /// The message up to the first line break.
    pub first_line: SharedString,
    /// Lines after the first.
    pub extra_lines: u32,
    pub full: Arc<str>,
    pub pid: Option<u32>,
    pub cursor: Arc<str>,
}

impl LogLine {
    pub fn from_entry(entry: &JournalEntry) -> Self {
        let mut time = String::with_capacity(19);
        local_time(entry.realtime_us / 1_000_000, &mut time);
        let (first_line, extra_lines) = split_message(&entry.message);
        Self {
            time: time.into(),
            prio_word: SharedString::new_static(priority_word(entry.priority)),
            priority: entry.priority,
            first_line: SharedString::from(first_line.to_string()),
            extra_lines,
            full: entry.message.clone(),
            pid: entry.pid,
            cursor: entry.cursor.clone(),
        }
    }

    /// True when the message or the priority word contains `needle`, ignoring case. `needle` is
    /// already lowercase.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || contains_ignore_case(&self.full, needle)
            || contains_ignore_case(&self.prio_word, needle)
    }
}

/// The syslog word of priority `p`. Values above 7 read as "debug".
pub fn priority_word(p: u8) -> &'static str {
    match p {
        0 => "emerg",
        1 => "alert",
        2 => "crit",
        3 => "err",
        4 => "warning",
        5 => "notice",
        6 => "info",
        _ => "debug",
    }
}

/// The colour role of priority `p`: 0 to 3 Danger, 4 Warning, 5 and 6 Primary, 7 Secondary.
pub fn priority_tone(p: u8) -> LabelTone {
    match p {
        0..=3 => LabelTone::Danger,
        4 => LabelTone::Warning,
        5 | 6 => LabelTone::Primary,
        _ => LabelTone::Secondary,
    }
}

/// Appends "<time> <priority> <message>" with the full multi-line message.
pub fn format_copy_line(l: &LogLine, out: &mut String) {
    out.push_str(&l.time);
    out.push(' ');
    out.push_str(&l.prio_word);
    out.push(' ');
    out.push_str(&l.full);
}

/// `arg` in double quotes for a command line, with `"`, `` ` ``, `$` and `\` escaped by a
/// backslash (the Desktop Entry `Exec` rule, which is also safe inside `sh` double quotes).
pub fn quote_exec_arg(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The command that opens `unit`'s journal for this boot in `terminal`.
pub fn terminal_command(terminal: &str, unit: &UnitRef) -> String {
    let flag = match unit.manager {
        Manager::System => "-u",
        Manager::User => "--user-unit",
    };
    format!(
        "{terminal} journalctl {flag} {} -b",
        quote_exec_arg(&unit.name)
    )
}

/// The first line of `message` and how many lines follow it.
fn split_message(message: &str) -> (&str, u32) {
    let mut lines = message.lines();
    let first = lines.next().unwrap_or("");
    (first, u32::try_from(lines.count()).unwrap_or(u32::MAX))
}

/// True when `hay` contains `needle`, ignoring case. `needle` is already lowercase. An ASCII
/// needle compares bytes without allocating.
fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        let n = needle.as_bytes();
        hay.as_bytes()
            .windows(n.len())
            .any(|w| w.eq_ignore_ascii_case(n))
    } else {
        hay.to_lowercase().contains(needle)
    }
}

/// "YYYY-MM-DD HH:MM:SS" in the system time zone.
fn local_time(unix_s: u64, out: &mut String) {
    let Some(z) = i64::try_from(unix_s)
        .ok()
        .and_then(|secs| jiff::Timestamp::from_second(secs).ok())
        .map(|t| t.to_zoned(jiff::tz::TimeZone::system()))
    else {
        return;
    };
    units::date_time(
        out,
        LocalDateTime {
            year: i32::from(z.year()),
            month: z.month() as u8,
            day: z.day() as u8,
            hour: z.hour() as u8,
            minute: z.minute() as u8,
            second: z.second() as u8,
        },
    );
}

/// The time colour and the priority and message colour of a line. The time is always
/// `text_secondary`. Danger and warning stay on a selected line.
fn line_colors(priority: u8, selected: bool, theme: &TlTheme) -> (Hsla, Hsla) {
    (
        theme.text_secondary,
        tone_color(priority_tone(priority), theme, selected),
    )
}

/// A toast in `window`'s own stack.
fn window_toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    match ToastQueue::of_window(window, cx) {
        Some(queue) => queue.update(cx, |q, cx| q.push(kind, text, None, None, cx)),
        None => warn!(target: "tasklens::ui", "no toast stack for the service Logs tab"),
    }
}

/// The shown lines in display order, for `Selection`.
struct Shown<'a>(&'a [u32]);

impl SelectionView<u32> for Shown<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn id(&self, ix: usize) -> u32 {
        self.0[ix]
    }

    fn index_of(&self, id: u32) -> Option<usize> {
        self.0.binary_search(&id).ok()
    }

    fn selectable(&self, _ix: usize) -> bool {
        true
    }
}

/// A terminal launch in flight: its action and which of `TERMINALS` it used.
#[derive(Clone, Copy, Debug)]
struct PendingLaunch {
    id: ActionId,
    attempt: usize,
}

pub struct LogsView {
    /// The host this tab shows. Set when it opens; never changes.
    host: HostContext,
    unit: UnitRef,
    /// Formatted once on arrival.
    lines: VecDeque<LogLine>,
    /// Indices into `lines` shown after the filter, ascending.
    filtered: Vec<u32>,
    /// The applied filter text.
    filter: SharedString,
    /// One handle for the window's life. Dropping the view drops it and the engine kills the
    /// child. A hidden tab turns follow off instead (`on_shown`).
    handle: Option<JournalHandle>,
    /// Default true.
    follow: bool,
    /// True while the list is within one line of the bottom. Batches then scroll to the newest
    /// line.
    stick_to_bottom: bool,
    /// Lines shown since the list unstuck, for the pill.
    new_since_unstick: u32,
    /// The first batch of this handle has arrived.
    first_batch: bool,
    /// The last journal error. It replaces the list until Retry.
    error: Option<JournalError>,
    /// The engine says only the user's own entries are readable.
    own_only: bool,
    /// A Load older request is in flight.
    loading_older: bool,
    /// The last older page was short: nothing older remains.
    exhausted: bool,
    /// The user scrolled with the wheel since the last render: re-read the position.
    scroll_dirty: bool,
    /// Line index selection.
    selection: Selection<u32>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    filter_input: Entity<InputState>,
    /// The filter debounce, restarted on every change.
    debounce: Option<Task<()>>,
    /// Reads `handle.events()`.
    _reader: Option<Task<()>>,
    /// Terminal launches in flight. Each click adds one, so each keeps its own fallback.
    launches: SmallVec<[PendingLaunch; 2]>,
    _subs: SmallVec<[Subscription; 2]>,
}

impl LogsView {
    pub(super) fn new(
        host: &HostContext,
        unit: UnitRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&filter_input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_filter_change(cx);
            }
        }));
        if let Some(tracker) = host.tracker().cloned() {
            subs.push(cx.subscribe_in(
                &tracker,
                window,
                |this, _, ev: &ActionTrackerEvent, window, cx| {
                    if let ActionTrackerEvent::Finished { id, ok, total, .. } = *ev {
                        this.on_launch_finished(id, ok == total && ok > 0, window, cx);
                    }
                },
            ));
        }
        let mut this = Self {
            host: host.clone(),
            unit,
            lines: VecDeque::new(),
            filtered: Vec::new(),
            filter: SharedString::default(),
            handle: None,
            follow: true,
            stick_to_bottom: true,
            new_since_unstick: 0,
            first_batch: false,
            error: None,
            own_only: false,
            loading_older: false,
            exhausted: false,
            scroll_dirty: false,
            selection: Selection::new(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            filter_input,
            debounce: None,
            _reader: None,
            launches: SmallVec::new(),
            _subs: subs,
        };
        this.open_journal(true, cx);
        this
    }

    /// Opens a journal handle and starts reading its events. The engine sends the last 200
    /// entries first.
    fn open_journal(&mut self, follow: bool, cx: &mut Context<Self>) {
        let Some(client) = self.host.hub().map(|hub| hub.read(cx).client().clone()) else {
            return;
        };
        let handle = client.journal(JournalQuery {
            unit: self.unit.clone(),
            follow,
        });
        let events = handle.events();
        self._reader = Some(cx.spawn(async move |this, cx| {
            while let Ok(ev) = events.recv().await {
                if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
        self.handle = Some(handle);
    }

    /// Retry: drops the handle and its reader, clears the list and opens a new handle.
    fn retry(&mut self, cx: &mut Context<Self>) {
        self._reader = None;
        self.handle = None;
        self.lines.clear();
        self.filtered.clear();
        self.selection = Selection::new();
        self.error = None;
        self.own_only = false;
        self.first_batch = false;
        self.loading_older = false;
        self.exhausted = false;
        self.new_since_unstick = 0;
        self.stick_to_bottom = true;
        self.open_journal(self.follow, cx);
        cx.notify();
    }

    fn list_state(&self) -> ListState {
        if let Some(e) = &self.error {
            ListState::Error(e.clone())
        } else if !self.first_batch {
            ListState::Loading
        } else if self.lines.is_empty() {
            ListState::NoEntries
        } else if self.filtered.is_empty() {
            ListState::NoMatch
        } else {
            ListState::List
        }
    }

    fn on_event(&mut self, ev: JournalEvent, cx: &mut Context<Self>) {
        match ev {
            JournalEvent::Batch {
                entries,
                older,
                exhausted,
            } => {
                if older {
                    self.loading_older = false;
                    self.exhausted = exhausted;
                } else {
                    self.first_batch = true;
                }
                let line_h = px(LINE_H * metrics(cx).ui_scale);
                self.take_batch(&entries, older, line_h);
            }
            JournalEvent::Access(access) => self.own_only = access == JournalAccess::OwnOnly,
            JournalEvent::Error(e) => {
                self.error = Some(e);
                self.loading_older = false;
            }
        }
        cx.notify();
    }

    /// Formats `entries` once and appends them, or prepends them for an `older` page. Only the
    /// new lines go through the filter. `line_h` keeps the scroll position on the same line.
    fn take_batch(&mut self, entries: &[JournalEntry], older: bool, line_h: Pixels) {
        if entries.is_empty() {
            return;
        }
        let needle = self.filter.to_lowercase();
        if older {
            let n = entries.len() as u32;
            for entry in entries.iter().rev() {
                self.lines.push_front(LogLine::from_entry(entry));
            }
            let mut filtered: Vec<u32> = (0..n)
                .filter(|&i| self.lines[i as usize].matches(&needle))
                .collect();
            let inserted_shown = filtered.len();
            filtered.extend(self.filtered.iter().map(|&i| i + n));
            self.filtered = filtered;
            // Line indices moved.
            self.selection.clear();
            self.shift_scroll(-(line_h * inserted_shown as f32));
            // Past the cap the newest lines go: the user is reading history.
            if self.lines.len() > MAX_LINES {
                self.lines.truncate(MAX_LINES);
                let keep = self.filtered.partition_point(|&i| (i as usize) < MAX_LINES);
                self.filtered.truncate(keep);
            }
        } else {
            let start = self.lines.len() as u32;
            self.lines.extend(entries.iter().map(LogLine::from_entry));
            let end = self.lines.len() as u32;
            let mut shown_new = 0;
            for i in start..end {
                if self.lines[i as usize].matches(&needle) {
                    self.filtered.push(i);
                    shown_new += 1;
                }
            }
            let excess = self.lines.len().saturating_sub(MAX_LINES);
            if excess > 0 {
                self.drop_oldest(excess, line_h);
            }
            if self.stick_to_bottom {
                self.scroll_to_bottom();
            } else {
                self.new_since_unstick = self.new_since_unstick.saturating_add(shown_new);
            }
        }
    }

    /// Drops the `count` oldest lines from `lines`, `filtered` and the selection, and keeps an
    /// unstuck list on the same line.
    fn drop_oldest(&mut self, count: usize, line_h: Pixels) {
        self.lines.drain(..count);
        let gone = self.filtered.partition_point(|&i| (i as usize) < count);
        self.filtered.drain(..gone);
        for i in &mut self.filtered {
            *i -= count as u32;
        }
        self.selection.clear();
        self.shift_scroll(line_h * gone as f32);
        // The engine can page back from the oldest kept line again (R-118).
        self.exhausted = false;
    }

    /// Moves the scroll offset by `dy` (negative scrolls down) and never above the top.
    fn shift_scroll(&self, dy: Pixels) {
        let base = self.scroll.0.borrow().base_handle.clone();
        let mut offset = base.offset();
        offset.y = (offset.y + dy).min(px(0.));
        base.set_offset(offset);
    }

    fn scroll_to_bottom(&self) {
        if let Some(last) = self.filtered.len().checked_sub(1) {
            self.scroll.scroll_to_item(last, ScrollStrategy::Bottom);
        }
    }

    /// Re-reads the scroll position after a wheel scroll: within one line of the bottom sticks.
    fn sync_stick(&mut self, line_h: Pixels) {
        let at_bottom = {
            let state = self.scroll.0.borrow();
            let max = state.base_handle.max_offset().y;
            max <= px(0.) || -state.base_handle.offset().y >= max - line_h
        };
        self.stick_to_bottom = at_bottom;
        if at_bottom {
            self.new_since_unstick = 0;
        }
    }

    /// The pill, End and the pill click: scroll to the newest line and stick.
    fn go_bottom(&mut self, cx: &mut Context<Self>) {
        self.stick_to_bottom = true;
        self.new_since_unstick = 0;
        self.scroll_to_bottom();
        cx.notify();
    }

    fn on_go_bottom(&mut self, _: &ServiceLogsBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.go_bottom(cx);
    }

    fn on_go_top(&mut self, _: &ServiceLogsTop, _: &mut Window, cx: &mut Context<Self>) {
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        if self.scroll.is_scrollable() {
            self.stick_to_bottom = false;
        }
        cx.notify();
    }

    fn toggle_follow(&mut self, cx: &mut Context<Self>) {
        self.follow = !self.follow;
        if let Some(handle) = &self.handle {
            handle.set_follow(self.follow);
        }
        if !self.follow {
            self.new_since_unstick = 0;
        }
        cx.notify();
    }

    /// Asks for the page before the oldest line held. One request at a time.
    fn load_older(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || self.exhausted {
            return;
        }
        let Some(handle) = &self.handle else {
            return;
        };
        self.loading_older = true;
        handle.load_older(self.lines.front().map(|l| l.cursor.clone()));
        cx.notify();
    }

    /// Restarts the debounce. The filter applies after 100 ms without a change.
    fn on_filter_change(&mut self, cx: &mut Context<Self>) {
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FILTER_DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                this.debounce = None;
                let text = SharedString::from(this.filter_input.read(cx).text().to_string());
                this.set_filter(text, cx);
            });
        }));
    }

    /// Applies `text` to the loaded lines: one linear scan.
    fn set_filter(&mut self, text: SharedString, cx: &mut Context<Self>) {
        if text == self.filter {
            return;
        }
        self.filter = text;
        let needle = self.filter.to_lowercase();
        self.filtered.clear();
        for (i, line) in self.lines.iter().enumerate() {
            if line.matches(&needle) {
                self.filtered.push(i as u32);
            }
        }
        let shown = Shown(&self.filtered);
        self.selection.retain(|id| shown.index_of(*id).is_some());
        cx.notify();
    }

    fn on_row_click(
        &mut self,
        pos: usize,
        ev: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(&id) = self.filtered.get(pos) else {
            return;
        };
        let mods = ev.modifiers();
        let shown = Shown(&self.filtered);
        match (mods.control || mods.platform, mods.shift) {
            (true, true) => self.selection.ctrl_shift_click(id, &shown),
            (true, false) => self.selection.ctrl_click(id),
            (false, true) => self.selection.shift_click(id, &shown),
            (false, false) => self.selection.click(id),
        }
        cx.notify();
    }

    fn on_select_all(&mut self, _: &ServiceLogsSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection.select_all(&Shown(&self.filtered));
        cx.notify();
    }

    fn on_copy_selected(
        &mut self,
        _: &ServiceLogsCopySelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selection.in_view_order(&Shown(&self.filtered));
        if ids.is_empty() {
            return;
        }
        let text = self.copy_text(ids.into_iter());
        self.copy(text, window, cx);
    }

    /// Copies every shown line.
    fn copy_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.copy_text(self.filtered.iter().copied());
        self.copy(text, window, cx);
    }

    /// The lines `ids` with `format_copy_line`, joined by "\n".
    fn copy_text(&self, ids: impl Iterator<Item = u32>) -> String {
        let mut out = String::new();
        for (n, id) in ids.enumerate() {
            if n > 0 {
                out.push('\n');
            }
            format_copy_line(&self.lines[id as usize], &mut out);
        }
        out
    }

    fn copy(&self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if crate::util::clipboard_write(cx, text).is_ok() {
            let toast = tr(cx, "sys-sys006-copied-to-clipboard");
            window_toast(window, cx, ToastKind::Success, toast);
        }
    }

    /// Opens the journal in a terminal with `TERMINALS[attempt]`.
    fn open_terminal(&mut self, attempt: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tracker) = self.host.tracker().cloned() else {
            warn!(target: "tasklens::ui", "no action tracker; the terminal was not opened");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
            warn!(target: "tasklens::ui", "no toast queue; the terminal was not opened");
            return;
        };
        let command = terminal_command(TERMINALS[attempt], &self.unit);
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::Launch(LaunchSpec {
                what: LaunchWhat::Command(command.into()),
                as_admin: false,
                cwd: None,
            }),
            targets: SmallVec::new(),
            privileged: false,
        };
        // The tab shows its own failure toast, and nothing on success.
        let texts = ToastTexts {
            owner_toasts: true,
            ..ToastTexts::new(SharedString::default(), SharedString::default())
        };
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if let Some(id) = sent {
            self.launches.push(PendingLaunch { id, attempt });
        }
    }

    fn on_launch_finished(
        &mut self,
        id: ActionId,
        ok: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.launches.iter().position(|p| p.id == id) else {
            return;
        };
        let pending = self.launches.swap_remove(at);
        if ok {
            return;
        }
        if pending.attempt + 1 < TERMINALS.len() {
            self.open_terminal(pending.attempt + 1, window, cx);
        } else {
            let text = tr(cx, "services-sys036-no-terminal");
            window_toast(window, cx, ToastKind::Error, text);
        }
    }

    fn older_label_key(&self) -> &'static str {
        if self.exhausted {
            "services-sys037-no-older"
        } else if self.loading_older {
            "services-sys037-loading"
        } else {
            "services-sys036-load-older"
        }
    }

    fn pill_text(&self, cx: &App) -> SharedString {
        let mut n = String::new();
        units::count(&mut n, u64::from(self.new_since_unstick));
        tr_args(
            cx,
            "services-sys037-new-lines",
            &[("n", FluentValue::from(n))],
        )
    }

    /// "↓ N new lines" while the list is unstuck with news and Follow is on.
    fn pill(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.follow || self.stick_to_bottom || self.new_since_unstick == 0 {
            return None;
        }
        let s = metrics(cx).ui_scale;
        let text = self.pill_text(cx);
        let this = cx.weak_entity();
        let button = TlButton::new("ServiceLogsNewLines", TlButtonKind::Primary)
            .size(TlButtonSize::Sm)
            .label(text)
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.go_bottom(cx));
            });
        Some(
            h_flex()
                .absolute()
                .bottom(px(PILL_BOTTOM * s))
                .left_0()
                .w_full()
                .justify_center()
                .child(button)
                .into_any_element(),
        )
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let filter = TlTextInput::new("ServiceLogsFilter", &self.filter_input)
            .size(TlInputSize::Sm)
            .prefix_icon(TlIcon::Search)
            .clear_button(true)
            .placeholder(tr(cx, "services-sys036-filter-placeholder"))
            .aria_label(tr(cx, "services-sys036-filter-placeholder"))
            .width(px(FILTER_W * s));
        let follow = TlButton::new(
            "ServiceLogsFollow",
            TlButtonKind::Segment { on: self.follow },
        )
        .icon(TlIcon::ArrowDownToLine)
        .icon_size(m.icon.sm)
        .label(tr(cx, "services-sys036-follow"))
        .on_click({
            let this = cx.weak_entity();
            move |_, _, cx| {
                let _ = this.update(cx, |this, cx| this.toggle_follow(cx));
            }
        });
        let older = TlButton::new("ServiceLogsLoadOlder", TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .label(tr(cx, self.older_label_key()))
            .disabled(self.loading_older || self.exhausted)
            .on_click({
                let this = cx.weak_entity();
                move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| this.load_older(cx));
                }
            });
        let this = cx.weak_entity();
        let copy = {
            let this = this.clone();
            TlButton::new("ServiceLogsCopy", TlButtonKind::Icon)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::Copy)
                .icon_size(m.icon.sm)
                .tooltip(tr(cx, "services-sys036-copy-tooltip"))
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |this, cx| this.copy_shown(window, cx));
                })
        };
        let terminal = TlButton::new("ServiceLogsTerminal", TlButtonKind::Icon)
            .size(TlButtonSize::Sm)
            .icon(TlIcon::Terminal)
            .icon_size(m.icon.sm)
            .tooltip(tr(cx, "services-sys036-terminal-tooltip"))
            .on_click(move |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.open_terminal(0, window, cx));
            });
        h_flex()
            .debug_selector(|| "ServiceLogsToolbar".into())
            .flex_none()
            .w_full()
            .h(px(TOOLBAR_H * s))
            .py(m.sp[2])
            .px(m.sp[4])
            .gap(m.sp[4])
            .items_center()
            .bg(theme.toolbar)
            .border_b_1()
            .border_color(theme.divider)
            .child(filter)
            .child(follow)
            .child(older)
            .child(div().flex_1())
            .child(copy)
            .child(terminal)
    }

    /// Noto Sans Mono at `size` on `line_h` lines.
    fn mono_style(size: Pixels, line_h: Pixels) -> TextStyleRefinement {
        TextStyleRefinement {
            font_family: Some(MONO_FAMILY.into()),
            font_size: Some(size.into()),
            font_weight: Some(FontWeight::NORMAL),
            line_height: Some(line_h.into()),
            ..Default::default()
        }
    }

    fn row(&self, pos: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let s = m.ui_scale;
        let id = self.filtered[pos];
        let line = &self.lines[id as usize];
        let selected = self.selection.contains(&id);
        let (time_color, text_color) = line_colors(line.priority, selected, &theme);
        let char_w = f32::from(m.text.sm) * MONO_ADVANCE;
        let line_h = px(LINE_H * s);
        let suffix = (line.extra_lines > 0).then(|| {
            let text = tr_args(
                cx,
                "services-sys036-more-lines",
                &[("n", FluentValue::from(i64::from(line.extra_lines)))],
            );
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_color(theme.text_secondary)
                .child(text)
        });
        let full = line.full.clone();
        let pid = line.pid;
        let message = h_flex()
            .id(("svc-log-msg", id as usize))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(text_color)
                    .child(line.first_line.clone()),
            )
            .children(suffix);
        let tip = HoverTooltip::state(("svc-log-tip", id as usize), window, cx);
        let message = HoverTooltip::attach(
            &tip,
            message,
            DELAY_ROW_NAME,
            move |_, cx| {
                let shown: String = full.chars().take(TOOLTIP_CHARS).collect();
                let mut lines: Vec<SharedString> = shown
                    .lines()
                    .map(|l| SharedString::from(l.to_string()))
                    .collect();
                if let Some(pid) = pid {
                    lines.push(tr_args(
                        cx,
                        "services-sys036-pid",
                        &[("pid", FluentValue::from(i64::from(pid)))],
                    ));
                }
                TlTooltip::lines(lines).view(cx)
            },
            window,
            cx,
        );
        let mut row = h_flex()
            .id(("svc-log-row", id as usize))
            .debug_selector(move || format!("ServiceLogs-row-{pos}"))
            .w_full()
            .h(line_h)
            .px(m.sp[4])
            .items_center()
            .role(Role::ListItem)
            .aria_label(line.first_line.clone())
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.on_row_click(pos, ev, window, cx)
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(TIME_CHARS * char_w))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(time_color)
                    .child(line.time.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(PRIO_CHARS * char_w))
                    .mx(m.sp[4])
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_color(text_color)
                    .child(line.prio_word.clone()),
            )
            .child(message);
        if selected {
            row = row.bg(theme.row_selected);
        }
        *row.text_style() = Self::mono_style(m.text.sm, line_h);
        row.into_any_element()
    }

    fn build_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let end = range.end.min(self.filtered.len());
        (range.start..end)
            .map(|pos| self.row(pos, window, cx))
            .collect()
    }

    /// The shown lines' text and colours, as the list draws them.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn rows_for_test(&self, cx: &App) -> Vec<LogRowForTest> {
        let theme = current(cx);
        self.filtered
            .iter()
            .map(|&id| {
                let line = &self.lines[id as usize];
                let selected = self.selection.contains(&id);
                let (time_color, text_color) = line_colors(line.priority, selected, theme);
                let suffix = (line.extra_lines > 0).then(|| {
                    tr_args(
                        cx,
                        "services-sys036-more-lines",
                        &[("n", FluentValue::from(i64::from(line.extra_lines)))],
                    )
                    .to_string()
                });
                LogRowForTest {
                    time: line.time.to_string(),
                    time_color,
                    priority: line.prio_word.to_string(),
                    message: line.first_line.to_string(),
                    text_color,
                    suffix,
                }
            })
            .collect()
    }

    /// The format of `LogLine::time`, for a test that builds the expected copy text.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn time_for_test(realtime_us: u64) -> String {
        let mut out = String::new();
        local_time(realtime_us / 1_000_000, &mut out);
        out
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn filter_input_for_test(&self) -> Entity<InputState> {
        self.filter_input.clone()
    }

    /// The scroll and paging state, as the tab shows it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn follow_state_for_test(&self, cx: &App) -> FollowStateForTest {
        let line_h = f32::from(px(LINE_H * metrics(cx).ui_scale));
        let base = self.scroll.0.borrow().base_handle.clone();
        FollowStateForTest {
            first_visible: (-f32::from(base.offset().y) / line_h).floor().max(0.) as usize,
            at_bottom: self.scroll.is_scrolled_to_end().unwrap_or(true),
            stick_to_bottom: self.stick_to_bottom,
            new_since_unstick: self.new_since_unstick,
            follow: self.follow,
            older_label: tr(cx, self.older_label_key()).to_string(),
            older_enabled: !self.loading_older && !self.exhausted,
            pill_text: self.pill_text(cx).to_string(),
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn focus_handle_for_test(&self) -> FocusHandle {
        self.focus.clone()
    }
}

/// Scroll and paging state, for tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug)]
pub struct FollowStateForTest {
    /// Index into the shown lines of the first line in view.
    pub first_visible: usize,
    pub at_bottom: bool,
    pub stick_to_bottom: bool,
    pub new_since_unstick: u32,
    pub follow: bool,
    pub older_label: String,
    pub older_enabled: bool,
    /// The pill's text for `new_since_unstick`, whether or not the pill shows.
    pub pill_text: String,
}

/// One shown line, for tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug)]
pub struct LogRowForTest {
    pub time: String,
    pub time_color: Hsla,
    pub priority: String,
    pub message: String,
    /// The colour of the priority word and the message.
    pub text_color: Hsla,
    /// " (+N lines)" for a multi-line message.
    pub suffix: Option<String>,
}

impl ServicePropsTabView for LogsView {
    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    fn apply(&mut self, _window: &mut Window, _cx: &mut App) -> SmallVec<[ActionId; 2]> {
        SmallVec::new()
    }

    fn reset(&mut self, _cx: &mut App) {}

    fn set_details(&mut self, _: Option<&Arc<UnitDetails>>, _: bool, _cx: &mut App) {}

    /// A hidden tab stops the `-f` child. Coming back resumes after the last cursor.
    fn on_shown(&mut self, shown: bool, _cx: &mut App) {
        if self.follow
            && let Some(handle) = &self.handle
        {
            handle.set_follow(shown);
        }
    }
}

impl LogsView {
    /// The centred text or error block for a state other than the list.
    fn state_element(state: &ListState, cx: &App) -> AnyElement {
        let retry = |s: TlEmptyState| {
            s.clear_filter(Box::new(ServiceLogsRetry))
                .button_label(tr(cx, "services-sys038-retry"))
        };
        match state {
            ListState::Error(JournalError::NotFound) => TlEmptyState::new("")
                .kit_icon(IconName::CircleAlert)
                .tone(TlEmptyTone::Warning)
                .heading(tr(cx, "services-sys038-not-found"))
                .into_any_element(),
            ListState::Error(JournalError::Timeout) => {
                retry(TlEmptyState::new("").heading(tr(cx, "services-sys038-timeout")))
                    .into_any_element()
            }
            ListState::Error(JournalError::Exit {
                stderr_first_line, ..
            }) => retry(
                TlEmptyState::new(SharedString::from(stderr_first_line.to_string()))
                    .heading(tr(cx, "services-sys038-read-failed"))
                    .one_line(),
            )
            .into_any_element(),
            ListState::Error(JournalError::Overflow) => {
                retry(TlEmptyState::new("").heading(tr(cx, "services-sys038-read-failed")))
                    .into_any_element()
            }
            ListState::Loading => {
                TlEmptyState::new(tr(cx, "services-sys038-loading")).into_any_element()
            }
            ListState::NoEntries => {
                TlEmptyState::new(tr(cx, "services-sys038-no-entries")).into_any_element()
            }
            ListState::NoMatch => {
                TlEmptyState::new(tr(cx, "services-sys038-no-match")).into_any_element()
            }
            ListState::List => div().into_any_element(),
        }
    }

    /// The list area's state, as the tab shows it.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn status_for_test(&self, cx: &App) -> LogsStatusForTest {
        let (heading, text, button) = match self.list_state() {
            ListState::Error(JournalError::NotFound) => {
                (Some(tr(cx, "services-sys038-not-found")), None, None)
            }
            ListState::Error(JournalError::Timeout) => (
                Some(tr(cx, "services-sys038-timeout")),
                None,
                Some(tr(cx, "services-sys038-retry")),
            ),
            ListState::Error(JournalError::Exit {
                stderr_first_line, ..
            }) => (
                Some(tr(cx, "services-sys038-read-failed")),
                Some(SharedString::from(stderr_first_line.to_string())),
                Some(tr(cx, "services-sys038-retry")),
            ),
            ListState::Error(JournalError::Overflow) => (
                Some(tr(cx, "services-sys038-read-failed")),
                None,
                Some(tr(cx, "services-sys038-retry")),
            ),
            ListState::Loading => (None, Some(tr(cx, "services-sys038-loading")), None),
            ListState::NoEntries => (None, Some(tr(cx, "services-sys038-no-entries")), None),
            ListState::NoMatch => (None, Some(tr(cx, "services-sys038-no-match")), None),
            ListState::List => (None, None, None),
        };
        LogsStatusForTest {
            banner: self
                .own_only
                .then(|| tr(cx, "services-sys038-own-only").to_string()),
            heading: heading.map(|s| s.to_string()),
            text: text.map(|s| s.to_string()),
            button: button.map(|s| s.to_string()),
        }
    }
}

/// The list area's state, for tests.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogsStatusForTest {
    pub banner: Option<String>,
    pub heading: Option<String>,
    pub text: Option<String>,
    pub button: Option<String>,
}

impl Render for LogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        if std::mem::take(&mut self.scroll_dirty) {
            self.sync_stick(px(LINE_H * metrics(cx).ui_scale));
        }
        let state = self.list_state();
        let (body, pill) = if state == ListState::List {
            let list = uniform_list(
                "svc-logs-list",
                self.filtered.len(),
                cx.processor(Self::build_rows),
            )
            .track_scroll(&self.scroll)
            .size_full();
            (list.into_any_element(), self.pill(cx))
        } else {
            (Self::state_element(&state, cx), None)
        };
        let banner = self.own_only.then(|| {
            div().flex_none().w_full().child(
                TlBanner::new(TlBannerKind::Info, tr(cx, "services-sys038-own-only"))
                    .button(
                        tr(cx, "services-sys038-copy-command"),
                        Box::new(ServiceLogsCopyCommand),
                    )
                    .link(
                        tr(cx, "services-sys034-additional-setup"),
                        Box::new(OpenAdditionalSetup {
                            focus_row: Some(SetupRow::JournalAccess),
                        }),
                    ),
            )
        });
        v_flex()
            .id("ServiceLogsTab")
            .debug_selector(|| "ServiceLogsTab".into())
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    // A button click does not move focus, so its action would miss this tab.
                    this.focus.focus(window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &ServiceLogsRetry, _, cx| this.retry(cx)))
            .on_action(cx.listener(|this, _: &ServiceLogsCopyCommand, window, cx| {
                let command = tr(cx, "setup-row-journal-command").to_string();
                this.copy(command, window, cx);
            }))
            .child(self.toolbar(cx))
            .children(banner)
            .child(
                div()
                    .id("ServiceLogsList")
                    .debug_selector(|| "ServiceLogsList".into())
                    .key_context(JOURNAL_LIST_CONTEXT)
                    .track_focus(&self.focus)
                    .role(Role::List)
                    .on_action(cx.listener(Self::on_select_all))
                    .on_action(cx.listener(Self::on_copy_selected))
                    .on_action(cx.listener(Self::on_go_top))
                    .on_action(cx.listener(Self::on_go_bottom))
                    .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                        this.scroll_dirty = true;
                        cx.notify();
                    }))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.list_bg)
                    .child(body)
                    .children(pill),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(priority: u8, message: &str) -> LogLine {
        let entry = JournalEntry {
            realtime_us: 1_700_000_000_000_000,
            priority,
            pid: Some(42),
            message: Arc::from(message),
            cursor: Arc::from("s=1"),
        };
        LogLine::from_entry(&entry)
    }

    #[test]
    fn priority_words_and_tones() {
        let words: Vec<_> = (0..=7).map(priority_word).collect();
        assert_eq!(
            words,
            [
                "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug"
            ]
        );
        assert_eq!(priority_tone(0), LabelTone::Danger);
        assert_eq!(priority_tone(3), LabelTone::Danger);
        assert_eq!(priority_tone(4), LabelTone::Warning);
        assert_eq!(priority_tone(5), LabelTone::Primary);
        assert_eq!(priority_tone(6), LabelTone::Primary);
        assert_eq!(priority_tone(7), LabelTone::Secondary);
    }

    #[test]
    fn split_counts_extra_lines() {
        assert_eq!(split_message("a\nb\nc"), ("a", 2));
        assert_eq!(split_message("one"), ("one", 0));
        assert_eq!(split_message(""), ("", 0));
        let l = line(6, "first\nsecond");
        assert_eq!(&*l.first_line, "first");
        assert_eq!(l.extra_lines, 1);
        assert_eq!(&*l.prio_word, "info");
        assert_eq!(l.time.len(), 19);
    }

    #[test]
    fn copy_line_keeps_full_message() {
        let l = line(3, "a\nb");
        let mut out = String::new();
        format_copy_line(&l, &mut out);
        assert_eq!(out, format!("{} err a\nb", l.time));
    }

    #[test]
    fn quoting_escapes_shell_chars() {
        assert_eq!(quote_exec_arg("sshd.service"), "\"sshd.service\"");
        assert_eq!(
            quote_exec_arg("a$b\"c`d\\e.service"),
            "\"a\\$b\\\"c\\`d\\\\e.service\""
        );
        let system = UnitRef {
            manager: Manager::System,
            name: Arc::from("sshd.service"),
        };
        assert_eq!(
            terminal_command(TERMINALS[0], &system),
            "xdg-terminal-exec journalctl -u \"sshd.service\" -b"
        );
        let user = UnitRef {
            manager: Manager::User,
            name: Arc::from("$(rm).service"),
        };
        assert_eq!(
            terminal_command(TERMINALS[1], &user),
            "konsole -e journalctl --user-unit \"\\$(rm).service\" -b"
        );
    }

    #[test]
    fn filter_matches_any_case() {
        assert!(line(6, "Connection TIMEOUT").matches("timeout"));
        assert!(line(4, "x").matches("warn"));
        assert!(!line(6, "x").matches("timeout"));
        assert!(line(6, "Ärger").matches("ärg"));
        assert!(line(6, "x").matches(""));
    }
}
