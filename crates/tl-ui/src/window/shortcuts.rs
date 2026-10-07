//! "tasklens Keyboard Shortcuts": every key binding, grouped by heading, with a search box
//! (UI-SHL-095, APP-030, APP-031, canonical table (b), R-083, R-120, R-220).
//!
//! A Tool window keyed `WindowKey::Shortcuts`; `OpenShortcuts` focuses it when open. The rows
//! come from the live keymap when the window opens: each `SHORTCUT_DOCS` entry takes the
//! bindings of its Actions in its key context, so an Action that is not bound in this phase has
//! no row, and the key caps always match what is bound. A binding goes to the first entry that
//! matches it, so it shows in one row only. Three rows have keys that are not GPUI bindings
//! (Hold Ctrl, a printable character, Shift+click and Ctrl+click); they carry literal key text.
//!
//! The rows are built once per open and again on a language change; the search filters indices
//! into them. The search box has focus on open. Up and Down scroll the list. Esc clears the
//! search, or closes the window when the search is empty; Ctrl+W closes it through the frame.

use gpui_kit::base::Root;
use gpui_kit::component::input::{Escape, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement,
    Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled, Subscription,
    Window, WindowHandle, div, point, px, size,
};
use smallvec::SmallVec;

use super::registry::{GlobalRegistry, WindowKey};
use crate::actions::OpenShortcuts;
use crate::app::AppEnv;
use crate::components::icons::TlIcon;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::window_frame::{
    CloseSecondaryWindow, TlSecondaryWindow, TlWindowKind, TlWindowSpec,
};
use crate::i18n::{Strings, tr};
use crate::text::style::{t_base, t_sm};
use crate::theme::{current, metrics};

/// Window size and minimum size, × ui_scale.
const SIZE: (f32, f32) = (560., 640.);
const MIN_SIZE: (f32, f32) = (420., 360.);
/// Row and heading height, × ui_scale.
const ROW: f32 = 28.;

const TITLE: &str = "shortcuts-title";
const PLACEHOLDER: &str = "shortcuts-search-placeholder";
const OR: &str = "shortcuts-or";

/// The headings in the order they are drawn (R-220). A heading with no row is not drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Heading {
    Global,
    Tables,
    Processes,
    History,
    Services,
    StartupApps,
    Connections,
    Information,
    Performance,
    /// PropsWindow, ServiceProps, AffinityDialog, PriorityDialog, SignalDialog.
    PropertiesWindows,
    /// RunDialog, Picker, ColumnPicker, HandleSearch, JournalList, LowMemory.
    Dialogs,
}

impl Heading {
    pub const ALL: [Heading; 11] = [
        Self::Global,
        Self::Tables,
        Self::Processes,
        Self::History,
        Self::Services,
        Self::StartupApps,
        Self::Connections,
        Self::Information,
        Self::Performance,
        Self::PropertiesWindows,
        Self::Dialogs,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Global => "shortcuts-heading-global",
            Self::Tables => "shortcuts-heading-tables",
            Self::Processes => "shortcuts-heading-processes",
            Self::History => "shortcuts-heading-history",
            Self::Services => "shortcuts-heading-services",
            Self::StartupApps => "shortcuts-heading-startup-apps",
            Self::Connections => "shortcuts-heading-connections",
            Self::Information => "shortcuts-heading-information",
            Self::Performance => "shortcuts-heading-performance",
            Self::PropertiesWindows => "shortcuts-heading-properties-windows",
            Self::Dialogs => "shortcuts-heading-dialogs",
        }
    }
}

/// The key contexts whose bindings the window lists, with their heading (R-120, R-220). A
/// binding counts by the first context of its predicate, so `HandleSearch > Table` is
/// HandleSearch.
pub const LISTED_CONTEXTS: &[(&str, Heading)] = &[
    ("Global", Heading::Global),
    ("Table", Heading::Tables),
    ("ProcessTable", Heading::Processes),
    ("History", Heading::History),
    ("ServiceTable", Heading::Services),
    ("StartupTable", Heading::StartupApps),
    ("ConnectionTable", Heading::Connections),
    ("Information", Heading::Information),
    ("Performance", Heading::Performance),
    ("PropsWindow", Heading::PropertiesWindows),
    ("ServiceProps", Heading::PropertiesWindows),
    ("AffinityDialog", Heading::PropertiesWindows),
    ("PriorityDialog", Heading::PropertiesWindows),
    ("SignalDialog", Heading::PropertiesWindows),
    ("RunDialog", Heading::Dialogs),
    ("Picker", Heading::Dialogs),
    ("ColumnPicker", Heading::Dialogs),
    ("ColumnPickerList", Heading::Dialogs),
    ("HandleSearch", Heading::Dialogs),
    ("JournalList", Heading::Dialogs),
    ("LowMemory", Heading::Dialogs),
];

/// The first context of a binding predicate: `HandleSearch` for `HandleSearch > Table`.
pub fn first_context(predicate: &str) -> &str {
    predicate.split(" > ").next().unwrap_or(predicate).trim()
}

/// One row of canonical table (b).
pub struct ShortcutDoc {
    /// Full Action names, for example `tasklens::SelectTab`.
    pub actions: &'static [&'static str],
    /// The binding's whole context predicate, for example `PropsWindow > KvGrid`.
    pub context: &'static str,
    /// Only bindings with these keystrokes (GPUI form, `alt-f`); empty for every binding.
    pub only: &'static [&'static str],
    /// Fluent keys of key text that is not a GPUI binding.
    pub literal: &'static [&'static str],
    pub heading: Heading,
    /// Fluent key of the description.
    pub text: &'static str,
    /// Shown only with `--debug`.
    pub debug_only: bool,
}

const fn doc(
    actions: &'static [&'static str],
    context: &'static str,
    heading: Heading,
    text: &'static str,
) -> ShortcutDoc {
    ShortcutDoc {
        actions,
        context,
        only: &[],
        literal: &[],
        heading,
        text,
        debug_only: false,
    }
}

const GLOBAL: &str = "Global";
const TABLE: &str = "Table";

/// One entry per row of canonical table (b), in drawing order within each heading. Actions of
/// later epics are matched by name; until they are bound their rows are absent.
pub const SHORTCUT_DOCS: &[ShortcutDoc] = &[
    // Global
    doc(
        &["tasklens::SelectTab"],
        GLOBAL,
        Heading::Global,
        "shortcuts-select-tab",
    ),
    doc(
        &["tasklens::NextTab", "tasklens::PrevTab"],
        GLOBAL,
        Heading::Global,
        "shortcuts-next-tab",
    ),
    doc(
        &["tasklens::OpenRunDialog"],
        GLOBAL,
        Heading::Global,
        "shortcuts-open-run-dialog",
    ),
    doc(
        &["tasklens::Quit"],
        GLOBAL,
        Heading::Global,
        "shortcuts-quit",
    ),
    doc(
        &["tasklens::CloseWindow"],
        GLOBAL,
        Heading::Global,
        "shortcuts-close-window",
    ),
    doc(
        &["tasklens::OpenSettings"],
        GLOBAL,
        Heading::Global,
        "shortcuts-open-settings",
    ),
    doc(
        &["tasklens::OpenShortcuts"],
        GLOBAL,
        Heading::Global,
        "shortcuts-open-shortcuts",
    ),
    ShortcutDoc {
        only: &["f10"],
        ..doc(
            &["tasklens::OpenMenu"],
            GLOBAL,
            Heading::Global,
            "shortcuts-open-file-menu",
        )
    },
    ShortcutDoc {
        only: &["alt-d"],
        debug_only: true,
        ..doc(
            &["tasklens::OpenMenu"],
            GLOBAL,
            Heading::Global,
            "shortcuts-open-debug-menu",
        )
    },
    doc(
        &["tasklens::OpenMenu"],
        GLOBAL,
        Heading::Global,
        "shortcuts-open-menu",
    ),
    doc(
        &["tasklens::FocusFilter"],
        GLOBAL,
        Heading::Global,
        "shortcuts-focus-filter",
    ),
    doc(
        &["tasklens::RefreshCurrentTab"],
        GLOBAL,
        Heading::Global,
        "shortcuts-refresh-current-tab",
    ),
    doc(
        &["tasklens::EscapeChain"],
        GLOBAL,
        Heading::Global,
        "shortcuts-escape-chain",
    ),
    ShortcutDoc {
        literal: &["shortcuts-key-hold-ctrl"],
        ..doc(&[], GLOBAL, Heading::Global, "shortcuts-pause-display")
    },
    doc(
        &[
            "tasklens::ScaleUp",
            "tasklens::ScaleDown",
            "tasklens::ScaleReset",
        ],
        GLOBAL,
        Heading::Global,
        "shortcuts-scale",
    ),
    doc(
        &["tasklens::ToggleMiniWindow"],
        GLOBAL,
        Heading::Global,
        "shortcuts-toggle-mini-window",
    ),
    // Tables
    ShortcutDoc {
        only: &[
            "shift-up",
            "shift-down",
            "shift-pageup",
            "shift-pagedown",
            "shift-home",
            "shift-end",
            "ctrl-space",
        ],
        literal: &["shortcuts-key-shift-click", "shortcuts-key-ctrl-click"],
        ..doc(
            &[
                "tasklens::MoveFocus",
                "tasklens::MoveFocusPage",
                "tasklens::MoveFocusEdge",
                "tasklens::ToggleSelectFocus",
            ],
            TABLE,
            Heading::Tables,
            "shortcuts-extend-selection",
        )
    },
    doc(
        &[
            "tasklens::MoveFocus",
            "tasklens::MoveFocusPage",
            "tasklens::MoveFocusEdge",
        ],
        TABLE,
        Heading::Tables,
        "shortcuts-move-focus",
    ),
    doc(
        &["tasklens::SelectAll"],
        TABLE,
        Heading::Tables,
        "shortcuts-select-all",
    ),
    doc(
        &["tasklens::ExpandOrChild", "tasklens::CollapseOrParent"],
        TABLE,
        Heading::Tables,
        "shortcuts-expand-or-child",
    ),
    doc(
        &["tasklens::ToggleExpand"],
        TABLE,
        Heading::Tables,
        "shortcuts-toggle-expand",
    ),
    doc(
        &["tasklens::OpenProperties"],
        TABLE,
        Heading::Tables,
        "shortcuts-open-properties",
    ),
    doc(
        &["tasklens::CopyRow"],
        TABLE,
        Heading::Tables,
        "shortcuts-copy-row",
    ),
    doc(
        &["tasklens::CopyRowsAsTable"],
        TABLE,
        Heading::Tables,
        "shortcuts-copy-rows-as-table",
    ),
    doc(
        &["tasklens::CollapseAll"],
        TABLE,
        Heading::Tables,
        "shortcuts-collapse-all",
    ),
    doc(
        &["tasklens::OpenContextMenu"],
        TABLE,
        Heading::Tables,
        "shortcuts-open-context-menu",
    ),
    ShortcutDoc {
        literal: &["shortcuts-key-printable"],
        ..doc(&[], TABLE, Heading::Tables, "shortcuts-start-filtering")
    },
    // Processes
    doc(
        &["tasklens::EndTask"],
        "ProcessTable",
        Heading::Processes,
        "shortcuts-end-task",
    ),
    doc(
        &["tasklens::EndProcess"],
        "ProcessTable",
        Heading::Processes,
        "shortcuts-end-process",
    ),
    doc(
        &["tasklens::CopyDetails"],
        "ProcessTable",
        Heading::Processes,
        "shortcuts-copy-details",
    ),
    // Performance
    doc(
        &["tasklens::PerfCopyDeviceInfo"],
        "Performance",
        Heading::Performance,
        "shortcuts-perf-copy-device-info",
    ),
    doc(
        &["tasklens::PerfToggleSummaryMode"],
        "Performance",
        Heading::Performance,
        "shortcuts-perf-toggle-summary-mode",
    ),
    doc(
        &["tasklens::PerfToggleSidebar"],
        "Performance",
        Heading::Performance,
        "shortcuts-perf-toggle-sidebar",
    ),
    doc(
        &["tasklens::PerfSelectVisibleDevice"],
        "Performance",
        Heading::Performance,
        "shortcuts-perf-select-visible-device",
    ),
    doc(
        &[
            "tasklens::PerfSelectPrevDevice",
            "tasklens::PerfSelectNextDevice",
            "tasklens::PerfSelectFirstDevice",
            "tasklens::PerfSelectLastDevice",
        ],
        "Performance",
        Heading::Performance,
        "shortcuts-perf-move-selection",
    ),
    // Properties windows
    doc(
        &["tasklens::PropsSelectTab"],
        "PropsWindow",
        Heading::PropertiesWindows,
        "shortcuts-props-select-tab",
    ),
    doc(
        &["tasklens::PropsNextTab", "tasklens::PropsPrevTab"],
        "PropsWindow",
        Heading::PropertiesWindows,
        "shortcuts-props-next-tab",
    ),
    doc(
        &["tasklens::PropsFocusFilter"],
        "PropsWindow",
        Heading::PropertiesWindows,
        "shortcuts-props-focus-filter",
    ),
    doc(
        &["tasklens::PropsRefreshNow"],
        "PropsWindow",
        Heading::PropertiesWindows,
        "shortcuts-props-refresh-now",
    ),
    doc(
        &[
            "tasklens::PropsFieldUp",
            "tasklens::PropsFieldDown",
            "tasklens::PropsFieldLeft",
            "tasklens::PropsFieldRight",
            "tasklens::PropsFieldHome",
            "tasklens::PropsFieldEnd",
            "tasklens::PropsActivateField",
            "tasklens::PropsCopyValue",
        ],
        "PropsWindow > KvGrid",
        Heading::PropertiesWindows,
        "shortcuts-props-field",
    ),
    doc(
        &[
            "tasklens::AffinityMoveLeft",
            "tasklens::AffinityMoveRight",
            "tasklens::AffinityMoveUp",
            "tasklens::AffinityMoveDown",
            "tasklens::AffinityHome",
            "tasklens::AffinityEnd",
            "tasklens::AffinityToggle",
        ],
        "AffinityDialog",
        Heading::PropertiesWindows,
        "shortcuts-affinity",
    ),
    doc(
        &["tasklens::PrioritySliderStep", "tasklens::PriorityApply"],
        "PriorityDialog",
        Heading::PropertiesWindows,
        "shortcuts-priority",
    ),
    doc(
        &["tasklens::SignalSendRealtime"],
        "SignalDialog",
        Heading::PropertiesWindows,
        "shortcuts-signal",
    ),
    // Dialogs
    doc(
        &[
            "tasklens::RunHistoryUp",
            "tasklens::RunHistoryDown",
            "tasklens::RunCommand",
            "tasklens::RunCommandElevated",
        ],
        "RunDialog",
        Heading::Dialogs,
        "shortcuts-run",
    ),
    doc(
        &["tasklens::CancelWindowPick"],
        "Picker",
        Heading::Dialogs,
        "shortcuts-cancel-window-pick",
    ),
    doc(
        &[
            "tasklens::ColumnPickerPrev",
            "tasklens::ColumnPickerNext",
            "tasklens::ColumnPickerToggle",
            "tasklens::ColumnPickerFirst",
            "tasklens::ColumnPickerLast",
        ],
        "ColumnPickerList",
        Heading::Dialogs,
        "shortcuts-column-picker",
    ),
    doc(
        &["tasklens::StartHandleSearch", "tasklens::StopHandleSearch"],
        "HandleSearch",
        Heading::Dialogs,
        "shortcuts-handle-search",
    ),
    doc(
        &[
            "tasklens::HandleSearchEndTask",
            "tasklens::HandleSearchEndProcess",
            "tasklens::HandleSearchGoToProcess",
            "tasklens::HandleSearchCopyPaths",
            "tasklens::HandleSearchCopyRows",
        ],
        "HandleSearch > Table",
        Heading::Dialogs,
        "shortcuts-handle-search-table",
    ),
    doc(
        &["tasklens::LowMemClearOrClose", "tasklens::LowMemClose"],
        "LowMemory",
        Heading::Dialogs,
        "shortcuts-low-memory",
    ),
];

/// One drawn row.
pub struct ShortcutRow {
    pub heading: Heading,
    pub description: SharedString,
    pub keys: SmallVec<[Kbd; 2]>,
    /// Key text that is not a binding ("Hold Ctrl").
    pub literal: SmallVec<[SharedString; 2]>,
    /// Every key, joined with " or ".
    pub key_text: SharedString,
    /// Lower-case description and key text.
    search_text: String,
    /// Indices into the keymap of the bindings this row shows.
    bindings: SmallVec<[usize; 4]>,
}

impl ShortcutRow {
    /// Indices into `cx.key_bindings()` of the bindings this row shows.
    pub fn binding_indices(&self) -> &[usize] {
        &self.bindings
    }

    fn matches(&self, needle: &str) -> bool {
        needle.is_empty() || self.search_text.contains(needle)
    }
}

/// Builds the rows from the live keymap, in heading order. `debug` shows the `--debug` rows.
pub fn build_rows(cx: &App, debug: bool) -> Vec<ShortcutRow> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    // The entry each listed binding goes to: the first one that matches it.
    let mut per_doc: Vec<SmallVec<[usize; 4]>> = vec![SmallVec::new(); SHORTCUT_DOCS.len()];
    for (ix, binding) in keymap.bindings().enumerate() {
        let Some(predicate) = binding.predicate() else {
            continue;
        };
        let predicate = predicate.to_string();
        let name = binding.action().name();
        let Some(stroke) = binding.keystrokes().first() else {
            continue;
        };
        let stroke = stroke.inner().unparse();
        let hit = SHORTCUT_DOCS.iter().position(|d| {
            d.context == predicate
                && d.actions.contains(&name)
                && (d.only.is_empty() || d.only.contains(&stroke.as_str()))
        });
        if let Some(d) = hit {
            per_doc[d].push(ix);
        }
    }

    let or = format!(" {} ", tr(cx, OR));
    let all: Vec<_> = keymap.bindings().collect();
    let mut rows = Vec::new();
    for heading in Heading::ALL {
        for (doc, bindings) in SHORTCUT_DOCS.iter().zip(&per_doc) {
            if doc.heading != heading
                || (doc.debug_only && !debug)
                || (bindings.is_empty() && doc.literal.is_empty())
            {
                continue;
            }
            let keys: SmallVec<[Kbd; 2]> = bindings
                .iter()
                .filter_map(|&ix| all[ix].keystrokes().first())
                .map(|k| Kbd::new(k.inner().clone()))
                .collect();
            let literal: SmallVec<[SharedString; 2]> =
                doc.literal.iter().map(|k| tr(cx, k)).collect();
            let mut key_text = String::new();
            let texts = bindings
                .iter()
                .filter_map(|&ix| all[ix].keystrokes().first())
                .map(|k| Kbd::format(k.inner()))
                .chain(literal.iter().map(|l| l.to_string()));
            for (i, t) in texts.enumerate() {
                if i > 0 {
                    key_text.push_str(&or);
                }
                key_text.push_str(&t);
            }
            let description = tr(cx, doc.text);
            let search_text = format!("{description} {key_text}").to_lowercase();
            rows.push(ShortcutRow {
                heading,
                description,
                keys,
                literal,
                key_text: key_text.into(),
                search_text,
                bindings: bindings.clone(),
            });
        }
    }
    rows
}

/// Registers the `OpenShortcuts` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: from a menu the main window is mid-update.
    cx.on_action(|_: &OpenShortcuts, cx| cx.defer(open_shortcuts));
}

/// Focuses the open window, or opens it. Without a window registry (tests) it always opens one.
pub fn open_shortcuts(cx: &mut App) {
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, TITLE),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(WindowKey::Shortcuts);
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, |window, cx| {
            cx.new(|cx| ShortcutsWindow::new(window, cx))
        })?;
        follow_title(handle, cx);
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Shortcuts, cx, open));
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// Sets the frame title again when the strings version changes, one `tr` call per version
/// (R-132).
fn follow_title(handle: WindowHandle<Root>, cx: &mut App) {
    let Some(frame) = TlSecondaryWindow::<ShortcutsWindow>::find(handle, cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        frame.update(cx, |_, cx| {
            let mut version = strings_version(cx);
            cx.observe_global_in::<Strings>(window, move |frame, window, cx| {
                let now = strings_version(cx);
                if now != version {
                    version = now;
                    frame.set_title(tr(cx, TITLE), window, cx);
                }
            })
            .detach();
        });
    });
}

fn strings_version(cx: &App) -> u64 {
    cx.try_global::<Strings>().map_or(0, |s| s.strings_version)
}

/// The window's own text, translated once per strings version (R-132).
struct ShortcutsWords {
    placeholder: SharedString,
    or: SharedString,
    headings: [SharedString; 11],
}

impl ShortcutsWords {
    fn new(cx: &App) -> Self {
        Self {
            placeholder: tr(cx, PLACEHOLDER),
            or: tr(cx, OR),
            headings: Heading::ALL.map(|h| tr(cx, h.key())),
        }
    }
}

/// The content of the Keyboard Shortcuts window.
pub struct ShortcutsWindow {
    search: Entity<InputState>,
    rows: Vec<ShortcutRow>,
    /// Indices into `rows` that match the search.
    filtered: Vec<usize>,
    /// The lower-case search text.
    needle: String,
    words: ShortcutsWords,
    strings_version: u64,
    debug: bool,
    scroll: ScrollHandle,
    _search: Subscription,
    _strings: Subscription,
}

impl ShortcutsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        let _search = cx.subscribe(&search, |this: &mut Self, search, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let needle = search.read(cx).text().to_string().to_lowercase();
                this.set_needle(needle);
                cx.notify();
            }
        });
        search.update(cx, |s, cx| s.focus(window, cx));
        let _strings = cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = strings_version(cx);
            if version != this.strings_version {
                this.strings_version = version;
                this.words = ShortcutsWords::new(cx);
                this.rows = build_rows(cx, this.debug);
                this.refilter();
                cx.notify();
            }
        });
        let debug = cx.try_global::<AppEnv>().is_some_and(|e| e.debug);
        let rows = build_rows(cx, debug);
        let mut this = Self {
            search,
            filtered: Vec::with_capacity(rows.len()),
            rows,
            needle: String::new(),
            words: ShortcutsWords::new(cx),
            strings_version: strings_version(cx),
            debug,
            scroll: ScrollHandle::new(),
            _search,
            _strings,
        };
        this.refilter();
        this
    }

    fn set_needle(&mut self, needle: String) {
        if needle != self.needle {
            self.needle = needle;
            self.refilter();
        }
    }

    fn refilter(&mut self) {
        self.filtered.clear();
        let needle = &self.needle;
        self.filtered.extend(
            self.rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.matches(needle))
                .map(|(i, _)| i),
        );
    }

    /// The search box state.
    pub fn search(&self) -> &Entity<InputState> {
        &self.search
    }

    /// Every row built at open, in drawing order.
    pub fn rows(&self) -> &[ShortcutRow] {
        &self.rows
    }

    /// The rows that match the search, in drawing order.
    pub fn visible_rows(&self) -> impl Iterator<Item = &ShortcutRow> {
        self.filtered.iter().map(|&i| &self.rows[i])
    }

    /// The headings drawn for the current search, in order.
    pub fn visible_headings(&self) -> Vec<Heading> {
        let mut out: Vec<Heading> = Vec::new();
        for row in self.visible_rows() {
            if out.last() != Some(&row.heading) {
                out.push(row.heading);
            }
        }
        out
    }

    /// The drawn name of `heading`.
    pub fn heading_name(&self, heading: Heading) -> &SharedString {
        let ix = Heading::ALL.iter().position(|h| *h == heading).unwrap_or(0);
        &self.words.headings[ix]
    }

    /// Scrolls the list by `rows` rows (negative: up).
    fn scroll_rows(&mut self, rows: f32, cx: &mut Context<Self>) {
        let step = ROW * metrics(cx).ui_scale;
        let offset = self.scroll.offset();
        let max = self.scroll.max_offset().y;
        let y = (offset.y - px(step * rows)).clamp(-max, px(0.));
        self.scroll.set_offset(point(offset.x, y));
        cx.notify();
    }
}

impl Render for ShortcutsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        let m = *metrics(cx);
        let row_h = px(ROW * m.ui_scale);

        let mut list = div()
            .id("Shortcuts-list")
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        let mut last: Option<Heading> = None;
        for &ix in &self.filtered {
            let row = &self.rows[ix];
            if last != Some(row.heading) {
                last = Some(row.heading);
                let mut heading = div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(row_h)
                    .px(m.sp[4])
                    .bg(theme.header)
                    .text_color(theme.text_secondary);
                *heading.text_style() = t_sm(&m);
                list = list.child(heading.child(self.heading_name(row.heading).clone()));
            }
            let mut caps = div().flex().flex_none().items_center().gap(m.sp[2]);
            let mut first = true;
            let mut sep = |caps: gpui_kit::Div| {
                if std::mem::take(&mut first) {
                    caps
                } else {
                    caps.child(
                        div()
                            .text_color(theme.text_secondary)
                            .child(self.words.or.clone()),
                    )
                }
            };
            for key in &row.keys {
                caps = sep(caps).child(key.clone());
            }
            for text in &row.literal {
                caps = sep(caps).child(div().text_color(theme.text_secondary).child(text.clone()));
            }
            let mut r = div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .justify_between()
                .gap(m.sp[4])
                .h(row_h)
                .px(m.sp[4])
                .text_color(theme.text_primary)
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .child(row.description.clone()),
                )
                .child(caps);
            *r.text_style() = t_base(&m);
            list = list.child(r);
        }

        div()
            .id("Shortcuts")
            .size_full()
            .flex()
            .flex_col()
            .gap(m.sp[4])
            .pt(m.sp[5])
            .bg(theme.list_bg)
            // Capture: the search box would take Up and Down as caret moves.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.scroll_rows(-1., cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.scroll_rows(1., cx);
                cx.stop_propagation();
            }))
            // Esc with an empty search closes; with text, the search box clears it.
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.search.read(cx).text().len() == 0 {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(CloseSecondaryWindow), cx);
                }
            }))
            .child(
                div().px(m.sp[5]).child(
                    TlTextInput::new("shortcuts-search", &self.search)
                        .size(TlInputSize::Md)
                        .prefix_icon(TlIcon::Search)
                        .clear_button(true)
                        .clean_on_escape(true)
                        .placeholder(self.words.placeholder.clone()),
                ),
            )
            .child(list)
    }
}
