//! Startup Properties window (UI-SYS-050).
//!
//! A read-only list of one startup entry's values, found by `StartupRef` in every Startup
//! snapshot. When the entry leaves the snapshot the window keeps the last values, greys them
//! and appends " (removed)" to the title. Paths and commands are shown, never logged (ADR-015).

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;
use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::base::Root;
use gpui_kit::component::h_flex;
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px, size,
};
use tl_model::snapshot::{ComponentMask, Impact, StartupEntry, StartupStatus};
use tl_model::table::StartupRef;
use tl_model::validity::Validity;
use tracing::warn;

use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::icons::TlIcon;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, UnsupportedWord, kv_words,
    shown_text_in,
};
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::host::{HostRegistry, with_host_name};
use crate::hub::{SnapshotChanged, SnapshotHub};
use crate::i18n::{tr, tr_args};
use crate::pages::startup::delegate::StartupWords;
use crate::theme::{current, metrics};
use crate::util::clipboard_write;
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, OpenOutcome, WindowKey};

/// Default window size at 100% scale.
const WINDOW_SIZE: (f32, f32) = (860., 520.);
/// Smallest window size at 100% scale.
const MIN_SIZE: (f32, f32) = (700., 360.);
/// Footer height at 100% scale.
const FOOTER_H: f32 = 48.;
/// Label column width at 100% scale. The longest label, "Can run / Can toggle / Can delete / File
/// exists", must fit in it.
pub const LABEL_W: f32 = 380.;
/// Row height at 100% scale.
const ROW_H: f32 = 28.;
/// Copy icon size and hit area at 100% scale.
const COPY_ICON: f32 = 14.;
const COPY_CELL: f32 = 20.;
/// The most lines the window shows.
const MAX_LINES: usize = 16;

/// One label and value.
#[derive(Clone)]
pub struct PropLine {
    pub label: SharedString,
    pub value: TlValue,
}

type Signature = Vec<(SharedString, SharedString, Validity)>;

/// The title, such as "Startup properties: Discord".
fn title_for(name: &str, removed: bool, cx: &App) -> SharedString {
    let key = if removed {
        "startup-props-title-removed"
    } else {
        "startup-props-title"
    };
    tr_args(cx, key, &[("name", FluentValue::from(name))])
}

fn not_reported() -> TlValue {
    TlValue {
        validity: Validity::Unsupported,
        word: Some(UnsupportedWord::NotReported),
        ..TlValue::new("")
    }
}

fn yes_no(cx: &App, value: bool) -> SharedString {
    tr(
        cx,
        if value {
            "startup-props-yes"
        } else {
            "startup-props-no"
        },
    )
}

fn joined(list: &[Arc<str>]) -> String {
    list.iter().map(|s| &**s).collect::<Vec<_>>().join(", ")
}

/// Builds the lines in UI-SYS-050 order. Optional lines only when set. `removed` marks every
/// value Stale, which the grid draws in `text_secondary`.
pub fn build_lines(entry: &StartupEntry, removed: bool, cx: &App) -> Vec<PropLine> {
    let words = StartupWords::new(cx);
    let line = |key: &str, value: TlValue| PropLine {
        label: tr(cx, key),
        value,
    };
    let text = |key: &str, value: &str| line(key, TlValue::new(value.to_owned()));
    let mut lines = Vec::with_capacity(MAX_LINES);
    lines.push(text("startup-props-label-name", &entry.name));
    let publisher = match &entry.publisher {
        None => not_reported(),
        Some(p) if &**p == "User" => TlValue::new(words.user.to_string()),
        Some(p) => TlValue::new(p.to_string()),
    };
    lines.push(line("startup-props-label-publisher", publisher));
    lines.push(text(
        "startup-props-label-status",
        &format!(
            "{} / {} / {}",
            words.status(entry.status),
            yes_no(cx, entry.status == StartupStatus::Enabled),
            entry.state_code.as_deref().unwrap_or("-")
        ),
    ));
    let mut impact = words.impact(entry.impact).to_string();
    if matches!(entry.impact, Impact::High | Impact::Medium | Impact::Low)
        && let Some(detail) = &entry.impact_detail
    {
        impact.push_str(&format!(" ({} ms)", detail.activation_ms));
    }
    lines.push(text("startup-props-label-impact", &impact));
    lines.push(text(
        "startup-props-label-type",
        &format!(
            "{} / {} / {}",
            words.kind(entry.kind),
            entry.source_word,
            entry.scope.label()
        ),
    ));
    lines.push(text("startup-props-label-location", &entry.location));
    lines.push(text("startup-props-label-source-path", &entry.source_path));
    let resolved = entry.resolved_path.as_deref().map_or_else(
        || tr(cx, "startup-copy-not-found").to_string(),
        str::to_owned,
    );
    lines.push(text("startup-props-label-resolved-path", &resolved));
    lines.push(text("startup-props-label-command", &entry.command));
    lines.push(text(
        "startup-props-label-flags",
        &format!(
            "{} / {} / {} / {}",
            yes_no(cx, entry.can_run),
            yes_no(cx, entry.can_toggle),
            yes_no(cx, entry.can_delete),
            yes_no(cx, entry.file_exists)
        ),
    ));
    if !entry.only_show_in.is_empty() {
        lines.push(text(
            "startup-props-label-only-show-in",
            &joined(&entry.only_show_in),
        ));
    }
    if !entry.not_show_in.is_empty() {
        lines.push(text(
            "startup-props-label-not-show-in",
            &joined(&entry.not_show_in),
        ));
    }
    if let Some(v) = &entry.try_exec {
        lines.push(text("startup-props-label-try-exec", v));
    }
    if let Some(v) = &entry.autostart_phase {
        lines.push(text("startup-props-label-phase", v));
    }
    if let Some(v) = &entry.generated_unit {
        lines.push(text("startup-props-label-generated-unit", v));
    }
    if let Some(v) = &entry.overrides {
        lines.push(text("startup-props-label-overrides", v));
    }
    if removed {
        for line in &mut lines {
            if line.value.validity == Validity::Valid {
                line.value.validity = Validity::Stale;
            }
        }
    }
    lines
}

/// What changes the look of the window: label, text and validity of each line.
fn signature(lines: &[PropLine]) -> Signature {
    lines
        .iter()
        .map(|l| (l.label.clone(), l.value.text.clone(), l.value.validity))
        .collect()
}

fn copy_value(value: &TlValue, window: &mut Window, cx: &mut App) {
    let words = kv_words(cx);
    let Some(text) = shown_text_in(value, &words) else {
        return;
    };
    if clipboard_write(cx, text.to_string()).is_ok() {
        let toast = tr(cx, "startup-props-copied");
        if let Some(queue) = ToastQueue::of_window(window, cx) {
            queue.update(cx, |q, cx| {
                q.push(ToastKind::Success, toast, None, None, cx)
            });
        }
    }
}

pub struct StartupPropertiesView {
    id: StartupRef,
    /// The last entry seen. Kept after the entry leaves the snapshot.
    entry: StartupEntry,
    /// The entries slice of the last snapshot looked at, for the pointer compare.
    seen: Arc<[StartupEntry]>,
    removed: bool,
    /// The remote host the entry is on, for the title; `None` on the local host.
    remote: Option<Arc<str>>,
    lines: Vec<PropLine>,
    sig: Signature,
    copy_focus: Vec<FocusHandle>,
    focus: FocusHandle,
    #[cfg(any(test, feature = "test-hooks"))]
    redraws: Cell<u32>,
    _subs: Vec<Subscription>,
}

/// Opens or focuses the window for `r`. A full cap shows a Warning toast in the main window.
pub fn open_startup_properties(r: StartupRef, hub: &Entity<SnapshotHub>, cx: &mut App) {
    open(r, hub, cx);
}

fn open(r: StartupRef, hub: &Entity<SnapshotHub>, cx: &mut App) -> OpenOutcome {
    let Some(registry) = cx.try_global::<GlobalRegistry>().map(|g| g.0.clone()) else {
        warn!(target: "tasklens::ui", "no window registry, startup properties not opened");
        return OpenOutcome::Failed;
    };
    let entries = Arc::clone(&hub.read(cx).current().startup.entries);
    let Some(entry) = entries.iter().find(|e| e.id == r).cloned() else {
        return OpenOutcome::Failed;
    };
    let s = metrics(cx).ui_scale;
    let key = WindowKey::StartupProperties(r);
    let remote = HostRegistry::remote_name_of(hub, cx);
    let spec = TlWindowSpec::new(
        TlWindowKind::Properties,
        with_host_name(&title_for(&entry.name, false, cx), remote.as_deref()),
        size(px(WINDOW_SIZE.0 * s), px(WINDOW_SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(key.clone());
    let hub = hub.clone();
    let outcome = registry.update(cx, |registry, cx| {
        registry.open_or_focus(key, cx, move |cx| {
            let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
                cx.new(|cx| StartupPropertiesView::new(entry, entries, &hub, window, cx))
            })?;
            Ok(handle.into())
        })
    });
    match &outcome {
        OpenOutcome::Opened | OpenOutcome::Focused => {}
        OpenOutcome::CapReached(_) => {
            let text = tr(cx, "startup-props-too-many-windows");
            push_main_toast(cx, ToastKind::Warning, text, None, None);
        }
        OpenOutcome::Failed => {
            warn!(target: "tasklens::ui", "startup properties window did not open");
        }
    }
    outcome
}

/// Opens the windows for `refs` in order and stops after the first full cap, so one toast shows.
pub fn open_startup_properties_all(
    refs: impl IntoIterator<Item = StartupRef>,
    hub: &Entity<SnapshotHub>,
    cx: &mut App,
) {
    for r in refs {
        if matches!(open(r, hub, cx), OpenOutcome::CapReached(_)) {
            break;
        }
    }
}

impl StartupPropertiesView {
    fn new(
        entry: StartupEntry,
        entries: Arc<[StartupEntry]>,
        hub: &Entity<SnapshotHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sub = cx.subscribe_in(
            hub,
            window,
            |this, hub, event: &SnapshotChanged, window, cx| {
                if event.mask.intersects(ComponentMask::STARTUP) {
                    let entries = Arc::clone(&hub.read(cx).current().startup.entries);
                    this.take_entries(entries, window, cx);
                }
            },
        );
        let lines = build_lines(&entry, false, cx);
        let sig = signature(&lines);
        let copy_focus = (0..MAX_LINES).map(|_| cx.focus_handle()).collect();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            id: entry.id.clone(),
            entry,
            seen: entries,
            removed: false,
            remote: HostRegistry::remote_name_of(hub, cx),
            lines,
            sig,
            copy_focus,
            focus,
            #[cfg(any(test, feature = "test-hooks"))]
            redraws: Cell::new(0),
            _subs: vec![sub],
        }
    }

    fn take_entries(
        &mut self,
        entries: Arc<[StartupEntry]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.removed || Arc::ptr_eq(&entries, &self.seen) {
            return;
        }
        self.seen = Arc::clone(&entries);
        match entries.iter().find(|e| e.id == self.id) {
            Some(entry) => {
                let lines = build_lines(entry, false, cx);
                let sig = signature(&lines);
                self.entry = entry.clone();
                if sig != self.sig {
                    self.lines = lines;
                    self.sig = sig;
                    self.set_title(window, cx);
                    self.redraw(cx);
                }
            }
            None => {
                self.removed = true;
                self.lines = build_lines(&self.entry, true, cx);
                self.sig = signature(&self.lines);
                self.set_title(window, cx);
                self.redraw(cx);
            }
        }
    }

    fn redraw(&self, cx: &mut Context<Self>) {
        #[cfg(any(test, feature = "test-hooks"))]
        self.redraws.set(self.redraws.get() + 1);
        cx.notify();
    }

    fn set_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title: SharedString = with_host_name(
            &title_for(&self.entry.name, self.removed, cx),
            self.remote.as_deref(),
        )
        .into();
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
    }

    /// Ctrl+C copies the line whose copy button has focus.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if event.keystroke.key != "c" || !m.control || m.shift || m.alt || m.platform {
            return;
        }
        let row = (0..self.lines.len()).find(|&i| self.copy_focus[i].is_focused(window));
        if let Some(line) = row.and_then(|row| self.lines.get(row)) {
            copy_value(&line.value, window, cx);
            cx.stop_propagation();
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn is_removed(&self) -> bool {
        self.removed
    }

    /// How many times the view asked for a redraw. The frame draws its content again on every
    /// frame, so this counts the view's own requests, which is what a snapshot can change.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn redraw_count(&self) -> u32 {
        self.redraws.get()
    }

    /// The lines as "label<TAB>value", the way the golden file stores them.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn line_texts(&self, words: &crate::components::kv_grid::KvWords) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|l| {
                shown_text_in(&l.value, words).map(|text| format!("{}\t{text}", l.label))
            })
            .collect()
    }

    /// How each value draws, for the removed-colour check.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn value_looks(
        &self,
        words: &crate::components::kv_grid::KvWords,
        theme: &crate::theme::TlTheme,
    ) -> Vec<crate::components::kv_grid::ValueLook> {
        self.lines
            .iter()
            .filter_map(|l| crate::components::kv_grid::value_look_in(&l.value, words, theme))
            .collect()
    }
}

impl Render for StartupPropertiesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = metrics(cx);
        let s = m.ui_scale;
        let row_h = px(ROW_H * s);
        let section = TlKvSection {
            title: None,
            blocks: vec![TlKvBlock {
                heading: None,
                rows: self
                    .lines
                    .iter()
                    .map(|l| TlKvRow {
                        label: l.label.clone(),
                        value: l.value.clone(),
                    })
                    .collect(),
                indent: Some(false),
            }],
        };
        let grid = TlKeyValueGrid::new("StartupPropsGrid", KvLayout::Inline)
            .sections(&[section])
            .row_height(row_h)
            .label_width(px(LABEL_W * s));

        let mut buttons = div().flex_none().flex().flex_col();
        for (i, line) in self.lines.iter().enumerate() {
            let handle = self.copy_focus[i].clone();
            let label = tr_args(
                cx,
                "startup-props-copy",
                &[("label", FluentValue::from(line.label.as_ref()))],
            );
            let this = cx.weak_entity();
            let lit = handle.contains_focused(window, cx);
            let button =
                TlButton::new(ElementId::from(("StartupPropsCopy", i)), TlButtonKind::Icon)
                    .icon(TlIcon::Copy)
                    .icon_size(px(COPY_ICON * s))
                    .icon_muted(true)
                    .size(TlButtonSize::Sm)
                    .tooltip(label.clone())
                    .accessible_name(label)
                    .focus_handle(handle)
                    .on_click(move |_, window, cx| {
                        let line = this
                            .upgrade()
                            .and_then(|view| view.read(cx).lines.get(i).cloned());
                        if let Some(line) = line {
                            copy_value(&line.value, window, cx);
                        }
                    });
            buttons = buttons.child(
                div()
                    .id(ElementId::from(("StartupPropsCopyCell", i)))
                    .debug_selector(move || format!("StartupPropsCopy-{i}"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(row_h)
                    .w(px(COPY_CELL * s))
                    .opacity(if lit { 1.0 } else { 0.6 })
                    .hover(|style| style.opacity(1.0))
                    .child(button),
            );
        }

        let footer = div()
            .debug_selector(|| "StartupPropsFooter".into())
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .px(m.sp[5])
            .h(px(FOOTER_H * s))
            .bg(theme.toolbar)
            .border_t_1()
            .border_color(theme.divider)
            .child(
                TlButton::new("StartupPropsClose", TlButtonKind::Secondary)
                    .label(tr(cx, "startup-props-close"))
                    .action(Box::new(CloseSecondaryWindow)),
            );

        div()
            .id("StartupPropertiesWindow")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(
                div()
                    .id("StartupPropsScroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .p(m.sp[6])
                    .child(
                        h_flex()
                            .items_start()
                            .gap(m.sp[3])
                            .child(div().flex_1().min_w_0().child(grid))
                            .child(buttons),
                    ),
            )
            .child(footer)
    }
}
