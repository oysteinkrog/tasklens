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
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, TextStyleRefinement,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, LaunchSpec, LaunchWhat};
use tl_model::journal::{JournalEntry, JournalEvent, JournalHandle, JournalQuery};
use tl_model::snapshot::UnitDetails;
use tl_model::table::{Manager, UnitRef};
use tl_model::units::{self, LocalDateTime};
use tracing::warn;

use super::ServicePropsTabView;
use crate::action_tracker::{ActionTrackerEvent, GlobalActionTracker, ToastTexts};
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::{DELAY_ROW_NAME, HoverTooltip, TlTooltip};
use crate::hub::GlobalHub;
use crate::i18n::{tr, tr_args};
use crate::pages::services::actions::{
    JOURNAL_LIST_CONTEXT, ServiceLogsCopySelected, ServiceLogsSelectAll,
};
use crate::pages::sys_status::{LabelTone, tone_color};
use crate::table::selection::{Selection, SelectionView};
use crate::text::fonts::MONO_FAMILY;
use crate::theme::{TlTheme, current, metrics};

/// Quiet time after the last keystroke before the filter applies.
pub const FILTER_DEBOUNCE: Duration = Duration::from_millis(100);
/// Toolbar height, × ui_scale.
const TOOLBAR_H: f32 = 32.;
/// Filter input width, × ui_scale.
const FILTER_W: f32 = 220.;
/// List line height, × ui_scale.
const LINE_H: f32 = 18.;
/// Noto Sans Mono advance width as a fraction of the font size (600 of 1000 units).
const MONO_ADVANCE: f32 = 0.6;
/// Width of the time column in characters ("YYYY-MM-DD HH:MM:SS").
const TIME_CHARS: f32 = 19.;
/// Width of the priority column in characters ("warning").
const PRIO_CHARS: f32 = 7.;
/// The message tooltip shows at most this many characters.
const TOOLTIP_CHARS: usize = 2000;

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

/// The time colour and the priority and message colour of a line. Danger and warning stay on a
/// selected line.
fn line_colors(priority: u8, selected: bool, theme: &TlTheme) -> (Hsla, Hsla) {
    (
        tone_color(LabelTone::Secondary, theme, selected),
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
    unit: UnitRef,
    /// Formatted once on arrival.
    lines: VecDeque<LogLine>,
    /// Indices into `lines` shown after the filter, ascending.
    filtered: Vec<u32>,
    /// The applied filter text.
    filter: SharedString,
    /// Held so the engine keeps the stream open. Follow and paging use it (follow bead).
    #[allow(dead_code)]
    handle: Option<JournalHandle>,
    /// Default true. The behaviour is the follow bead's.
    follow: bool,
    #[allow(dead_code)]
    stick_to_bottom: bool,
    #[allow(dead_code)]
    new_since_unstick: u32,
    /// Line index selection.
    selection: Selection<u32>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    filter_input: Entity<InputState>,
    /// The filter debounce, restarted on every change.
    debounce: Option<Task<()>>,
    /// Reads `handle.events()`.
    _reader: Option<Task<()>>,
    launch: Option<PendingLaunch>,
    _subs: SmallVec<[Subscription; 2]>,
}

impl LogsView {
    pub(super) fn new(unit: UnitRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&filter_input, |this, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_filter_change(cx);
            }
        }));
        if let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) {
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
        let client = cx
            .try_global::<GlobalHub>()
            .map(|hub| hub.0.read(cx).client().clone());
        let handle = client.map(|client| {
            client.journal(JournalQuery {
                unit: unit.clone(),
                follow: true,
            })
        });
        let reader = handle.as_ref().map(|handle| {
            let events = handle.events();
            cx.spawn(async move |this, cx| {
                while let Ok(ev) = events.recv().await {
                    if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                        break;
                    }
                }
            })
        });
        Self {
            unit,
            lines: VecDeque::new(),
            filtered: Vec::new(),
            filter: SharedString::default(),
            handle,
            follow: true,
            stick_to_bottom: true,
            new_since_unstick: 0,
            selection: Selection::new(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            filter_input,
            debounce: None,
            _reader: reader,
            launch: None,
            _subs: subs,
        }
    }

    fn on_event(&mut self, ev: JournalEvent, cx: &mut Context<Self>) {
        // Access and Error are the states bead's.
        if let JournalEvent::Batch { entries, older, .. } = ev {
            self.take_batch(&entries, older);
            cx.notify();
        }
    }

    /// Formats `entries` once and appends them, or prepends them for an `older` page. Only the
    /// new lines go through the filter.
    fn take_batch(&mut self, entries: &[JournalEntry], older: bool) {
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
            filtered.extend(self.filtered.iter().map(|&i| i + n));
            self.filtered = filtered;
            // Line indices moved.
            self.selection.clear();
        } else {
            let start = self.lines.len() as u32;
            self.lines.extend(entries.iter().map(LogLine::from_entry));
            let end = self.lines.len() as u32;
            for i in start..end {
                if self.lines[i as usize].matches(&needle) {
                    self.filtered.push(i);
                }
            }
        }
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
        let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
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
        self.launch = sent.map(|id| PendingLaunch { id, attempt });
    }

    fn on_launch_finished(
        &mut self,
        id: ActionId,
        ok: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.launch.filter(|p| p.id == id) else {
            return;
        };
        self.launch = None;
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
        .label(tr(cx, "services-sys036-follow"));
        let older = TlButton::new("ServiceLogsLoadOlder", TlButtonKind::Secondary)
            .size(TlButtonSize::Sm)
            .label(tr(cx, "services-sys036-load-older"));
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
                .text_color(tone_color(LabelTone::Secondary, &theme, selected))
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
                    .flex_shrink()
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

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn focus_handle_for_test(&self) -> FocusHandle {
        self.focus.clone()
    }
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
}

impl Render for LogsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let list = uniform_list(
            "svc-logs-list",
            self.filtered.len(),
            cx.processor(Self::build_rows),
        )
        .track_scroll(&self.scroll)
        .size_full();
        v_flex()
            .id("ServiceLogsTab")
            .debug_selector(|| "ServiceLogsTab".into())
            .size_full()
            .child(self.toolbar(cx))
            .child(
                div()
                    .id("ServiceLogsList")
                    .debug_selector(|| "ServiceLogsList".into())
                    .key_context(JOURNAL_LIST_CONTEXT)
                    .track_focus(&self.focus)
                    .role(Role::List)
                    .on_action(cx.listener(Self::on_select_all))
                    .on_action(cx.listener(Self::on_copy_selected))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .bg(theme.list_bg)
                    .child(list),
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
