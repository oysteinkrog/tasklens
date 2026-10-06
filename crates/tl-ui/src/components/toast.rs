//! `ToastQueue`: the per-window toast stack every surface pushes to (UI-FND-020, UI-SHL-100,
//! APP-044, R-109, R-113, R-195, R-197, R-232).
//!
//! The queue owns every rule: at most 4 visible toasts (Progress toasts do not count), a FIFO wait
//! list of 16, the durations, hover and focus pause, replace in place by key, coalescing of
//! repeats, Error priority and the hidden-window rule. Owners choose only the kind, the text, the
//! key and an optional action button (R-113). Owners never push from `render`.
//!
//! The toasts are drawn by the kit notification list in the window's kit Root. Each toast is a kit
//! `Notification` with `autohide(false)` whose content is built from the queue on every render,
//! so a replace or a repeat changes the toast in place without moving it. The queue runs every
//! timer itself: the kit's 5 s autohide cannot restart on a replace without a re-push, and a
//! re-push moves the toast to the end of the stack and replays its enter animation.
//!
//! Every kit call is deferred to the end of the current update, so an owner may push from inside
//! a window event handler of the same window.
//!
//! Known kit limits in P1: the kit draws its own small close button on hover in the toast's top
//! right corner next to ours, keeps its own toast shadow, and animates enter and exit for 400 ms
//! and 200 ms (R-109). GPUI skips those animations when `cx.reduce_motion()` is set; the kit has
//! no switch for `motion.animations = false`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::theme::Theme as KitTheme;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    Action, Anchor, AnyElement, AnyWindowHandle, App, Context, ElementId, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled, StyledText, Task, TextRun, WeakEntity, Window, div,
    font, px,
};
use smallvec::SmallVec;
use tracing::{debug, warn};

use super::button::{TlButton, TlButtonKind, TlButtonSize};
use super::icons::{TlIcon, icon};
use super::progress::TlSpinner;
use super::tooltip::{DELAY_CONTROL, HoverTooltip, TlTooltip};
use crate::hub::GlobalHub;
use crate::text::fonts::UI_FAMILY;
use crate::text::style::{TNUM, t_base};
use crate::theme::{TlTheme, current, metrics};

/// Keys at or above this are fixed keys owned by pages. Tracker keys (`id << 8 | index`, with
/// `ActionId` below 2^55) never reach it (R-232).
pub const FIXED_KEY_BASE: u64 = 0x8000_0000_0000_0000;
/// The Services page's loading toast (R-232, E27).
pub const SERVICES_LOADING_TOAST: ToastKey = ToastKey(0x8000_0000_0000_0101);
/// The Startup Apps page's loading toast (R-232, E28).
pub const STARTUP_LOADING_TOAST: ToastKey = ToastKey(0x8000_0000_0000_0201);
/// The Connections page's loading toast (R-232, E28).
pub const CONNECTIONS_LOADING_TOAST: ToastKey = ToastKey(0x8000_0000_0000_0301);

/// Visible toasts, not counting Progress toasts.
pub const MAX_VISIBLE: usize = 4;
/// Toasts waiting for a free slot.
pub const MAX_WAITING: usize = 16;
/// Toast width in px.
pub const WIDTH: f32 = 382.0;
/// Minimum toast height before the UI scale.
pub const MIN_HEIGHT: f32 = 40.0;
/// Text lines shown before the cut.
pub const MAX_LINES: usize = 3;

/// Info, Success and Warning toasts.
pub const SHORT: Duration = Duration::from_secs(5);
/// Error toasts.
pub const ERROR: Duration = Duration::from_secs(8);
/// Any toast with an action button, except Progress.
pub const WITH_ACTION: Duration = Duration::from_secs(15);
/// A repeat of a visible toast within this time is counted instead of added.
pub const COALESCE_WINDOW: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
    /// An action in flight. Never auto-dismissed and never takes one of the 4 slots (R-197).
    Progress,
}

impl ToastKind {
    /// The kind spelled out, the accessible prefix of the tooltip ("Error: …").
    pub fn name(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Success => "Success",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Progress => "Progress",
        }
    }

    /// Info and Success toasts may be dropped from the wait list or evicted by an Error.
    fn is_minor(self) -> bool {
        matches!(self, Self::Info | Self::Success)
    }
}

/// An owner's toast identity. Pushing again with the same key replaces the toast in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ToastKey(pub u64);

/// The kit notification id type for keyed toasts: `Notification::id1::<ToastKeyTag>(key.0)`.
pub struct ToastKeyTag;

/// The kit notification id type for toasts pushed without a key.
struct AnonToastTag;

/// An optional button under the text. It dispatches `action`, then closes the toast.
pub struct ToastAction {
    pub label: SharedString,
    pub action: Box<dyn Action>,
}

impl Clone for ToastAction {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            action: self.action.boxed_clone(),
        }
    }
}

/// Which toast an entry is: an owner key or a queue-made number for an unkeyed push.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Key(u64),
    Anon(u64),
}

impl Slot {
    fn kit_note(self) -> Notification {
        match self {
            Self::Key(k) => Notification::new().id1::<ToastKeyTag>(k),
            Self::Anon(n) => Notification::new().id1::<AnonToastTag>(n),
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(k) => write!(f, "k{k}"),
            Self::Anon(n) => write!(f, "a{n}"),
        }
    }
}

struct ToastEntry {
    slot: Slot,
    kind: ToastKind,
    text: SharedString,
    action: Option<ToastAction>,
    /// Pushes coalesced into this toast, 1 for the first.
    count: u32,
    /// The last push that landed on this toast, for coalescing.
    last_push: Instant,
    /// Time left before the toast closes. `None` for Progress.
    remaining: Option<Duration>,
    /// When the running timer started. `None` while paused or not shown.
    since: Option<Instant>,
    timer: Option<Task<()>>,
    /// Bumped each time the entry is shown, so a late kit close of an older toast is ignored.
    shown: u64,
    focus: Option<FocusHandle>,
}

impl ToastEntry {
    fn duration(&self) -> Option<Duration> {
        match self.kind {
            ToastKind::Progress => None,
            _ if self.action.is_some() => Some(WITH_ACTION),
            ToastKind::Error => Some(ERROR),
            _ => Some(SHORT),
        }
    }

    /// The shown text, with " (×N)" from the second repeat on.
    fn display_text(&self) -> SharedString {
        if self.count >= 2 {
            format!("{}{}", self.text, count_suffix(self.count)).into()
        } else {
            self.text.clone()
        }
    }
}

fn count_suffix(count: u32) -> String {
    format!(" (×{count})")
}

/// What one toast's content needs from the queue, copied out for a render.
struct ToastView {
    kind: ToastKind,
    text: SharedString,
    count: u32,
    action: Option<ToastAction>,
    focused: bool,
}

/// The per-window toast stack. Create one per window with [`ToastQueue::new`]; the window must
/// have a kit Root.
pub struct ToastQueue {
    /// Shown toasts in stack order, oldest first. Includes Progress toasts.
    visible: SmallVec<[ToastEntry; 4]>,
    waiting: VecDeque<ToastEntry>,
    top: Pixels,
    window: AnyWindowHandle,
    next_anon: u64,
    next_shown: u64,
    /// Toasts under the pointer.
    hovered: SmallVec<[Slot; 2]>,
    /// Toasts holding keyboard focus.
    focused: SmallVec<[Slot; 2]>,
    paused: bool,
}

impl ToastQueue {
    /// A queue for `window`, with the stack's top edge at `top`. Writes the kit notification
    /// settings. In P1 every window passes the main-window top (R-195).
    pub fn new(window: &mut Window, cx: &mut Context<Self>, top: Pixels) -> Self {
        let settings = &mut KitTheme::global_mut(cx).notification;
        settings.placement = Anchor::TopCenter;
        settings.margins.top = top;
        // The queue enforces the cap of 4 itself, so the kit never hides a Progress toast.
        settings.max_items = MAX_VISIBLE + MAX_WAITING;
        settings.width = px(WIDTH);
        Self {
            visible: SmallVec::new(),
            waiting: VecDeque::new(),
            top,
            window: window.window_handle(),
            next_anon: 0,
            next_shown: 0,
            hovered: SmallVec::new(),
            focused: SmallVec::new(),
            paused: false,
        }
    }

    /// The stack's top edge.
    pub fn top(&self) -> Pixels {
        self.top
    }

    /// Moves the stack's top edge. The shell calls it again on every UI scale change
    /// (UI-SHL-130). `Theme::notification` is app-global, so the last call wins in every window.
    pub fn set_top(&mut self, top: Pixels, cx: &mut App) {
        self.top = top;
        KitTheme::global_mut(cx).notification.margins.top = top;
        cx.refresh_windows();
    }

    /// Shows a toast, or queues it when 4 are visible. See UI-FND-020 for the rules.
    pub fn push(
        &mut self,
        kind: ToastKind,
        text: impl Into<SharedString>,
        key: Option<ToastKey>,
        action: Option<ToastAction>,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        let now = cx.background_executor().now();
        debug!(?kind, ?key, "toast push");

        if main_window_hidden(cx) {
            if kind == ToastKind::Error {
                self.deliver_system(text, key, cx);
            }
            return;
        }

        if let Some(key) = key {
            let slot = Slot::Key(key.0);
            if let Some(ix) = self.visible.iter().position(|e| e.slot == slot) {
                self.replace_visible(ix, kind, text, action, now, cx);
                return;
            }
            if let Some(entry) = self.waiting.iter_mut().find(|e| e.slot == slot) {
                entry.kind = kind;
                entry.text = text;
                entry.action = action;
                entry.count = 1;
                entry.last_push = now;
                return;
            }
        } else if let Some(entry) = self.visible.iter_mut().find(|e| {
            e.kind == kind
                && e.text == text
                && now.saturating_duration_since(e.last_push) <= COALESCE_WINDOW
        }) {
            entry.count += 1;
            entry.last_push = now;
            let slot = entry.slot;
            self.restart(slot, now, cx);
            cx.refresh_windows();
            return;
        }

        let slot = match key {
            Some(key) => Slot::Key(key.0),
            None => {
                self.next_anon += 1;
                Slot::Anon(self.next_anon)
            }
        };
        let entry = ToastEntry {
            slot,
            kind,
            text,
            action,
            count: 1,
            last_push: now,
            remaining: None,
            since: None,
            timer: None,
            shown: 0,
            focus: None,
        };

        if kind == ToastKind::Progress || self.counted() < MAX_VISIBLE {
            self.show(entry, now, cx);
        } else if kind == ToastKind::Error
            && let Some(ix) = self.oldest_minor_visible(None)
        {
            let evicted = self.visible[ix].slot;
            self.take_visible(evicted, true, cx);
            self.show(entry, now, cx);
        } else {
            self.wait(entry);
        }
    }

    /// Closes the toast with `key`, or drops it from the wait list.
    pub fn remove(&mut self, key: ToastKey, cx: &mut Context<Self>) {
        debug!(?key, "toast remove");
        self.close(Slot::Key(key.0), cx);
    }

    /// The shown texts in stack order, oldest first, with any count suffix.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn visible_texts(&self) -> Vec<SharedString> {
        self.visible.iter().map(ToastEntry::display_text).collect()
    }

    /// The waiting texts, next to show first.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn waiting_texts(&self) -> Vec<SharedString> {
        self.waiting.iter().map(ToastEntry::display_text).collect()
    }

    /// Whether every visible timer is paused by hover or focus.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Visible toasts that hold one of the 4 slots.
    fn counted(&self) -> usize {
        self.visible
            .iter()
            .filter(|e| e.kind != ToastKind::Progress)
            .count()
    }

    fn oldest_minor_visible(&self, except: Option<usize>) -> Option<usize> {
        self.visible
            .iter()
            .enumerate()
            .find(|(ix, e)| Some(*ix) != except && e.kind.is_minor())
            .map(|(ix, _)| ix)
    }

    fn wait(&mut self, entry: ToastEntry) {
        self.waiting.push_back(entry);
        if self.waiting.len() > MAX_WAITING {
            match self.waiting.iter().position(|e| e.kind.is_minor()) {
                Some(ix) => {
                    let dropped = self.waiting.remove(ix);
                    if let Some(e) = dropped {
                        debug!(kind = ?e.kind, slot = %e.slot, "toast wait list full, dropped");
                    }
                }
                // Warnings and Errors are never dropped, so the list grows past 16 here.
                None => warn!(
                    len = self.waiting.len(),
                    "toast wait list holds no Info or Success"
                ),
            }
        }
    }

    fn show(&mut self, mut entry: ToastEntry, now: Instant, cx: &mut Context<Self>) {
        self.next_shown += 1;
        entry.shown = self.next_shown;
        entry.remaining = entry.duration();
        entry.focus = Some(cx.focus_handle());
        self.arm(&mut entry, now, cx);
        self.kit_push(&entry, cx);
        self.visible.push(entry);
    }

    fn replace_visible(
        &mut self,
        ix: usize,
        kind: ToastKind,
        text: SharedString,
        action: Option<ToastAction>,
        now: Instant,
        cx: &mut Context<Self>,
    ) {
        let was_progress = self.visible[ix].kind == ToastKind::Progress;
        let entry = &mut self.visible[ix];
        entry.kind = kind;
        entry.text = text;
        entry.action = action;
        entry.count = 1;
        entry.last_push = now;
        let slot = entry.slot;
        self.restart(slot, now, cx);

        let is_progress = kind == ToastKind::Progress;
        if was_progress && !is_progress && self.counted() > MAX_VISIBLE {
            // A Progress toast became a result and now takes a slot. It keeps its place; the
            // oldest other Info or Success makes room when there is one.
            if let Some(evict) = self.oldest_minor_visible(Some(ix)) {
                let evicted = self.visible[evict].slot;
                self.take_visible(evicted, true, cx);
            }
        } else if !was_progress && is_progress {
            self.promote(now, cx);
        }
        cx.refresh_windows();
    }

    /// Shows waiting toasts while slots are free.
    fn promote(&mut self, now: Instant, cx: &mut Context<Self>) {
        while self.counted() < MAX_VISIBLE {
            let Some(entry) = self.waiting.pop_front() else {
                break;
            };
            self.show(entry, now, cx);
        }
    }

    fn close(&mut self, slot: Slot, cx: &mut Context<Self>) {
        if self.take_visible(slot, true, cx) {
            let now = cx.background_executor().now();
            self.promote(now, cx);
        } else if let Some(ix) = self.waiting.iter().position(|e| e.slot == slot) {
            self.waiting.remove(ix);
        }
    }

    /// Takes the visible toast `slot` out of the queue. With `kit`, also closes the kit toast.
    fn take_visible(&mut self, slot: Slot, kit: bool, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.visible.iter().position(|e| e.slot == slot) else {
            return false;
        };
        self.visible.remove(ix);
        self.hovered.retain(|s| *s != slot);
        self.focused.retain(|s| *s != slot);
        self.update_pause(cx);
        if kit {
            self.kit_remove(slot, cx);
        }
        true
    }

    /// A timer ran out.
    fn expire(&mut self, slot: Slot, cx: &mut Context<Self>) {
        self.close(slot, cx);
    }

    /// The kit closed a toast: its own close button, or the end of an exit we started.
    fn kit_closed(&mut self, slot: Slot, shown: u64, cx: &mut Context<Self>) {
        let current = self
            .visible
            .iter()
            .any(|e| e.slot == slot && e.shown == shown);
        if current && self.take_visible(slot, false, cx) {
            let now = cx.background_executor().now();
            self.promote(now, cx);
        }
    }

    /// The close button or the action button was pressed.
    fn dismiss(&mut self, slot: Slot, cx: &mut Context<Self>) {
        self.close(slot, cx);
    }

    fn set_hovered(&mut self, slot: Slot, hovered: bool, cx: &mut Context<Self>) {
        set_member(&mut self.hovered, slot, hovered);
        self.update_pause(cx);
    }

    fn set_focused(&mut self, slot: Slot, focused: bool, cx: &mut Context<Self>) {
        set_member(&mut self.focused, slot, focused);
        self.update_pause(cx);
    }

    /// Pauses every visible timer while a toast is hovered or focused, and resumes them after.
    fn update_pause(&mut self, cx: &mut Context<Self>) {
        let paused = !self.hovered.is_empty() || !self.focused.is_empty();
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        let now = cx.background_executor().now();
        let mut visible = std::mem::take(&mut self.visible);
        for entry in &mut visible {
            if paused {
                disarm(entry, now);
            } else {
                self.arm(entry, now, cx);
            }
        }
        self.visible = visible;
    }

    /// Restarts the full duration of the visible toast `slot`.
    fn restart(&mut self, slot: Slot, now: Instant, cx: &mut Context<Self>) {
        let Some(ix) = self.visible.iter().position(|e| e.slot == slot) else {
            return;
        };
        let mut entry = self.visible.remove(ix);
        disarm(&mut entry, now);
        entry.remaining = entry.duration();
        self.arm(&mut entry, now, cx);
        self.visible.insert(ix, entry);
    }

    /// Starts the entry's timer for its remaining time, unless paused or a Progress toast.
    fn arm(&self, entry: &mut ToastEntry, now: Instant, cx: &mut Context<Self>) {
        if self.paused {
            return;
        }
        let Some(remaining) = entry.remaining else {
            return;
        };
        let slot = entry.slot;
        entry.since = Some(now);
        entry.timer = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(remaining).await;
            this.update(cx, |queue, cx| queue.expire(slot, cx)).ok();
        }));
    }

    fn view(&self, slot: Slot) -> Option<ToastView> {
        self.visible
            .iter()
            .find(|e| e.slot == slot)
            .map(|e| ToastView {
                kind: e.kind,
                text: e.text.clone(),
                count: e.count,
                action: e.action.clone(),
                focused: self.focused.contains(&slot),
            })
    }

    /// Adds the kit toast for `entry` after the current update.
    fn kit_push(&self, entry: &ToastEntry, cx: &mut Context<Self>) {
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let queue = cx.entity().downgrade();
        let closed = queue.clone();
        let slot = entry.slot;
        let shown = entry.shown;
        let focus = entry.focus.clone().unwrap_or_else(|| cx.focus_handle());
        let note = slot
            .kit_note()
            .autohide(false)
            .min_h(px(MIN_HEIGHT * m.ui_scale))
            .p(m.sp[4])
            .rounded(m.radius_card)
            .bg(theme.menu_bg)
            .border_color(theme.divider)
            .content(move |_, window, cx| {
                render_content(&queue, slot, &focus, window, cx)
                    .unwrap_or_else(|| div().into_any_element())
            })
            .on_close(move |_, cx| {
                if let Some(queue) = closed.upgrade() {
                    queue.update(cx, |queue, cx| queue.kit_closed(slot, shown, cx));
                }
            });
        let handle = self.window;
        cx.defer(move |cx| {
            if handle
                .update(cx, |_, window, cx| window.push_notification(note, cx))
                .is_err()
            {
                warn!(slot = %slot, "toast window is gone");
            }
        });
    }

    /// Closes the kit toast `slot` after the current update.
    fn kit_remove(&self, slot: Slot, cx: &mut Context<Self>) {
        let handle = self.window;
        cx.defer(move |cx| {
            handle
                .update(cx, |_, window, cx| match slot {
                    Slot::Key(k) => window.remove_notification1::<ToastKeyTag>(k, cx),
                    Slot::Anon(n) => window.remove_notification1::<AnonToastTag>(n, cx),
                })
                .ok();
        });
    }

    /// Sends an Error to the desktop while the main window is hidden (rule 8).
    fn deliver_system(&self, text: SharedString, key: Option<ToastKey>, cx: &mut Context<Self>) {
        let note = match key {
            Some(key) => Slot::Key(key.0).kit_note(),
            None => Notification::new(),
        }
        .message(text)
        .with_type(NotificationType::Error)
        .system();
        let handle = self.window;
        cx.defer(move |cx| {
            if handle
                .update(cx, |_, window, cx| window.push_notification(note, cx))
                .is_err()
            {
                warn!(
                    ?key,
                    "system notification not delivered: the window is gone"
                );
            }
        });
    }
}

fn set_member(set: &mut SmallVec<[Slot; 2]>, slot: Slot, on: bool) {
    let has = set.contains(&slot);
    if on && !has {
        set.push(slot);
    } else if !on && has {
        set.retain(|s| *s != slot);
    }
}

/// Stops the entry's timer and keeps the time left.
fn disarm(entry: &mut ToastEntry, now: Instant) {
    if let (Some(since), Some(remaining)) = (entry.since.take(), entry.remaining.as_mut()) {
        *remaining = remaining.saturating_sub(now.saturating_duration_since(since));
    }
    entry.timer = None;
}

/// Whether the hub reports the main window hidden (ADR-010).
fn main_window_hidden(cx: &App) -> bool {
    cx.try_global::<GlobalHub>()
        .is_some_and(|hub| hub.0.read(cx).is_hidden())
}

/// The leading icon of `kind`.
fn kind_icon(kind: ToastKind, theme: &TlTheme, size: Pixels) -> AnyElement {
    let (name, color) = match kind {
        ToastKind::Info => (TlIcon::Info, theme.status.info),
        ToastKind::Success => (TlIcon::CircleCheck, theme.status.success),
        ToastKind::Warning => (TlIcon::TriangleAlert, theme.status.warning),
        ToastKind::Error => (TlIcon::CircleX, theme.status.danger),
        ToastKind::Progress => {
            return TlSpinner::new(size)
                .color(theme.text_secondary)
                .into_any_element();
        }
    };
    icon(name, size, color).into_any_element()
}

/// One toast's content: icon, text with the optional action button under it, and the close
/// button. Built on every render of the kit toast from the queue's current entry.
fn render_content(
    queue: &WeakEntity<ToastQueue>,
    slot: Slot,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let queue = queue.upgrade()?;
    let view = queue.read(cx).view(slot)?;
    let m = *metrics(cx);
    let theme = current(cx).clone();

    let focused = focus.contains_focused(window, cx);
    if focused != view.focused {
        let queue = queue.clone();
        cx.defer(move |cx| {
            queue.update(cx, |queue, cx| queue.set_focused(slot, focused, cx));
        });
    }

    let text: AnyElement = if view.count >= 2 {
        // The count is a separate run with tabular figures.
        let suffix = count_suffix(view.count);
        let label_font = font(UI_FAMILY);
        let mut count_font = label_font.clone();
        count_font.features = TNUM.clone();
        let run = |len, font| TextRun {
            len,
            font,
            color: theme.text_primary,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = vec![
            run(view.text.len(), label_font),
            run(suffix.len(), count_font),
        ];
        StyledText::new(SharedString::from(format!("{}{suffix}", view.text)))
            .with_runs(runs)
            .into_any_element()
    } else {
        view.text.clone().into_any_element()
    };
    let mut text_box = div()
        .debug_selector(move || format!("TlToast-{slot}-text"))
        .min_w_0()
        .text_color(theme.text_primary);
    *text_box.text_style() = t_base(&m);
    let text_box = text_box.line_clamp(MAX_LINES).text_ellipsis().child(text);

    let full = if view.count >= 2 {
        format!("{}{}", view.text, count_suffix(view.count))
    } else {
        view.text.to_string()
    };
    let tip_text = format!("{}: {full}", view.kind.name());
    let tip_id = ElementId::NamedChild(
        Arc::new(ElementId::Name(format!("TlToast-{slot}").into())),
        "tooltip".into(),
    );
    let tip_state = HoverTooltip::state(tip_id, window, cx);
    let text_box = HoverTooltip::attach(
        &tip_state,
        div().relative().flex().min_w_0().child(text_box),
        DELAY_CONTROL,
        TlTooltip::text(tip_text).build(),
        window,
        cx,
    );

    let button = |queue: &gpui_kit::Entity<ToastQueue>| {
        let queue = queue.downgrade();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            if let Some(queue) = queue.upgrade() {
                queue.update(cx, |queue, cx| queue.dismiss(slot, cx));
            }
        }
    };
    let action = view.action.map(|a| {
        div().flex().child(
            TlButton::new(
                ElementId::Name(format!("TlToast-{slot}-action").into()),
                TlButtonKind::Secondary,
            )
            .size(TlButtonSize::Sm)
            .label(a.label)
            .action(a.action)
            .on_click(button(&queue)),
        )
    });
    let close = TlButton::new(
        ElementId::Name(format!("TlToast-{slot}-close").into()),
        TlButtonKind::Icon,
    )
    .size(TlButtonSize::Sm)
    .icon(TlIcon::X)
    .tooltip("Close")
    .on_click(button(&queue));

    let hover_queue = queue.downgrade();
    Some(
        h_flex()
            .id(ElementId::Name(format!("TlToast-{slot}").into()))
            .debug_selector(move || format!("TlToast-{slot}"))
            .track_focus(focus)
            .w_full()
            .items_start()
            .gap(m.sp[4])
            .on_hover(move |hovered, _, cx| {
                if let Some(queue) = hover_queue.upgrade() {
                    queue.update(cx, |queue, cx| queue.set_hovered(slot, *hovered, cx));
                }
            })
            .child(
                div()
                    .flex_none()
                    .pt(m.sp[1])
                    .child(kind_icon(view.kind, &theme, m.icon.md)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(m.sp[3])
                    .child(text_box)
                    .children(action),
            )
            .child(div().flex_none().child(close))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_keys_above_tracker_range() {
        for key in [
            SERVICES_LOADING_TOAST,
            STARTUP_LOADING_TOAST,
            CONNECTIONS_LOADING_TOAST,
        ] {
            assert!(key.0 >= FIXED_KEY_BASE, "{key:?}");
        }
        let highest_tracker_key = ((1u64 << 55) - 1) << 8 | 255;
        assert!(highest_tracker_key < FIXED_KEY_BASE);
    }

    #[test]
    fn count_suffix_uses_times_sign() {
        assert_eq!(count_suffix(5), " (×5)");
    }
}
