//! The Strings tab of Process Properties: an on-demand scan of process memory for text, with
//! options, progress, cancel and a virtualized result table of up to 1,000,000 rows
//! (UI-PWN-017, UI-PWN-018, UI-PWN-005, PROC-084, R-004, R-119, ADR-008).
//!
//! The scan never starts by itself: only Scan (the button, or Enter in the options bar) sends
//! `Command::StringScan`, each time with a new `ScanId`. A payload whose `ScanId` is not the
//! current one is dropped. Esc cancels a running scan before it clears the filter or closes the
//! window; `PropsWindow` asks [`StringsTab::scan_running`] for that. Switching tabs keeps the scan
//! running; dropping the tab (the window closed) sends `CancelStringScan`.
//!
//! The hits are never copied. The table reads them through the payload's chunk list and a chunk
//! index of start offsets, and every new payload hands over its chunk list as is (the engine
//! reuses the old chunk `Arc`s). The row id is the hit index. Sorting (after the scan) and
//! filtering (Value only) run on the background executor and give `Arc<[u32]>` lists; the view is
//! the sort permutation filtered by the match list.
//!
//! While scanning, a payload re-renders only the status row and the table's row count: the tab
//! itself re-renders only when the scan phase changes.
//!
//! Values are untrusted memory content and may hold secrets: they are never logged, and the cells
//! show them escaped (`\t`, `\n`, `\r`) and cut at 512 characters.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use gpui_kit::component::input::InputState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyView, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, WeakEntity, Window, div, prelude::FluentBuilder as _, px,
};
use smallvec::SmallVec;
use tl_model::command::Command;
use tl_model::details::{
    SCAN_PTRACE_BLOCKED, ScanId, ScanPhase, StringEncoding, StringHit, StringScanOpts,
    StringScanState, TabPayload,
};
use tl_model::ids::ProcessKey;
use tl_model::request::{IdKind, RequestEnd, RequestEvent};
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{FilterChanged, NO_MATCH, PropsFilter, SYNC_MAX_ROWS, lowercase};
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::checkbox::TlCheckbox;
use crate::components::icons::{TlIcon, icon};
use crate::components::menu::TlMenu;
use crate::components::progress::{TlProgress, TlProgressSize};
use crate::components::stepper::TlStepper;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::table::actions::CopyRow;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};
use crate::text::style::{numeric_style, t_base, t_sm};
use crate::theme::{current, metrics};
use crate::windows::actions::{
    PropsCancelStringScan, PropsStartStringScan, STRINGS_OPTIONS_CONTEXT,
};

gpui_kit::actions!(
    tasklens_strings,
    [StringsCopyValue, StringsCopyAddress, StringsCopyRow]
);

/// A Value cell shows this many characters, then "…".
pub const VALUE_CUT: usize = 512;
/// The next scan id. Process-global, so two windows never share one.
static NEXT_SCAN: AtomicU64 = AtomicU64::new(1);

/// The `Failed` reason of a scan the engine lost or refused. Never shown: the tab shows the
/// failed texts.
const SCAN_LOST: &str = "engine lost the scan";
/// Options bar height, × ui_scale.
const OPTIONS_H: f32 = 32.;
/// Status row and limit banner height, × ui_scale.
const STATUS_H: f32 = 24.;
/// Progress bar width, × ui_scale.
const PROGRESS_W: f32 = 200.;
const SCAN_ICON: f32 = 14.;
const CANCEL_ICON: f32 = 12.;
const BANNER_ICON: f32 = 14.;
/// Limit banner fill alpha of `status.warning`.
const BANNER_FILL_ALPHA: f32 = 0.15;

/// The columns, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StringsColumn {
    Address,
    Encoding,
    Length,
    Value,
}

impl StringsColumn {
    pub const ALL: [StringsColumn; 4] = [Self::Address, Self::Encoding, Self::Length, Self::Value];

    /// Header key, width at 100% and alignment. Value is the flex column: 480 is its start.
    fn meta(self) -> (&'static str, f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Address => ("pwn-strings-col-address", 140., Right),
            Self::Encoding => ("pwn-strings-col-encoding", 90., Left),
            Self::Length => ("pwn-strings-col-length", 70., Right),
            Self::Value => ("pwn-strings-col-value", 480., Left),
        }
    }
}

impl TableColumn for StringsColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Address => "address",
            Self::Encoding => "encoding",
            Self::Length => "length",
            Self::Value => "value",
        }
    }

    /// The layout is never saved, so no stored key maps to it.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }

    fn min_width(self) -> Option<f32> {
        Some(match self {
            Self::Address => 110.,
            Self::Encoding => 70.,
            Self::Length => 60.,
            Self::Value => 200.,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StringsSort {
    pub column: StringsColumn,
    pub ascending: bool,
}

impl Default for StringsSort {
    /// Address ascending: the order the hits arrive in.
    fn default() -> Self {
        Self {
            column: StringsColumn::Address,
            ascending: true,
        }
    }
}

impl TableSort<StringsColumn> for StringsSort {
    fn column(&self) -> StringsColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

// ---- Words (R-132) ----

/// The tab's words, translated once per strings version.
pub struct StringsWords {
    strings_version: Option<u64>,
    pub headers: [SharedString; 4],
    /// "ASCII", "UTF-8", "UTF-16LE": the checkbox labels and the Encoding cells.
    pub encodings: [SharedString; 3],
    pub min_length: SharedString,
    pub include_file_backed: SharedString,
    pub scan: SharedString,
    pub cancel: SharedString,
    pub cancel_tip: SharedString,
    pub no_encoding: SharedString,
    pub not_running: SharedString,
    pub idle: SharedString,
    pub limit: SharedString,
    pub failed: SharedString,
    pub kernel_thread: SharedString,
    pub ptrace_blocked: SharedString,
    pub sort_after_scan: SharedString,
    pub no_match: SharedString,
    pub copy_value: SharedString,
    pub copy_address: SharedString,
    pub copy_row: SharedString,
    pub copied: SharedString,
    progress: Template,
    finished: Template,
    cancelled: Template,
    exited: Template,
    stopped: Template,
}

impl StringsWords {
    fn with(
        strings_version: Option<u64>,
        t: impl Fn(&str) -> SharedString,
        template: impl Fn(&str, &[&str]) -> Template,
    ) -> Self {
        Self {
            strings_version,
            headers: StringsColumn::ALL.map(|c| t(c.meta().0)),
            encodings: [
                t("pwn-strings-enc-ascii"),
                t("pwn-strings-enc-utf8"),
                t("pwn-strings-enc-utf16le"),
            ],
            min_length: t("pwn-strings-min-length"),
            include_file_backed: t("pwn-strings-include-file-backed"),
            scan: t("pwn-strings-scan"),
            cancel: t("pwn-strings-cancel"),
            cancel_tip: t("pwn-strings-cancel-tooltip"),
            no_encoding: t("pwn-strings-no-encoding"),
            not_running: t("pwn-props-not-running"),
            idle: t("pwn-strings-idle"),
            limit: t("pwn-strings-limit"),
            failed: t("pwn-strings-failed"),
            kernel_thread: t("pwn-strings-kernel-thread"),
            ptrace_blocked: t("pwn-strings-ptrace-blocked"),
            sort_after_scan: t("pwn-strings-sort-after-scan"),
            no_match: t(NO_MATCH),
            copy_value: t("pwn-strings-menu-copy-value"),
            copy_address: t("pwn-strings-menu-copy-address"),
            copy_row: t("pwn-strings-menu-copy-row"),
            copied: t("pwn-strings-toast-copied"),
            progress: template("pwn-strings-progress", &["pct", "n"]),
            finished: template("pwn-strings-finished", &["n"]),
            cancelled: template("pwn-strings-cancelled", &["n"]),
            exited: template("pwn-strings-exited", &["n"]),
            stopped: template("pwn-strings-stopped", &["n"]),
        }
    }

    /// The words in en-US, without the strings global.
    pub fn en() -> Self {
        Self::with(None, |key| tr_en(key, &[]), Template::en)
    }

    fn translated(cx: &App) -> Self {
        Self::with(
            cx.try_global::<Strings>().map(|s| s.strings_version),
            |key| tr(cx, key),
            |key, args| Template::new(cx, key, args),
        )
    }

    /// The word for `enc`.
    pub fn encoding(&self, enc: StringEncoding) -> &SharedString {
        &self.encodings[match enc {
            StringEncoding::Ascii => 0,
            StringEncoding::Utf8 => 1,
            StringEncoding::Utf16Le => 2,
        }]
    }

    /// The status text, such as "Scanning process memory... 42%  18 234 strings found". Counts
    /// use the thin-space grouping of table d.
    pub fn status_text(&self, phase: &ScanPhase, pct: u8, found: u32) -> SharedString {
        let mut n = String::new();
        units::count(&mut n, u64::from(found));
        let mut out = String::new();
        let _ = match phase {
            ScanPhase::Scanning => self.progress.write(&mut out, &[&pct.min(100), &n]),
            ScanPhase::Finished => self.finished.write(&mut out, &[&n]),
            ScanPhase::Cancelled => self.cancelled.write(&mut out, &[&n]),
            ScanPhase::StoppedExited => self.exited.write(&mut out, &[&n]),
            ScanPhase::StoppedLimit | ScanPhase::Failed(_) => self.stopped.write(&mut out, &[&n]),
        };
        out.into()
    }
}

// ---- Values ----

/// Appends `value` with tabs, newlines and carriage returns as `\t`, `\n` and `\r`. With `cut`,
/// only the first `cut` characters, then "…" when there were more.
fn escape_into(out: &mut String, value: &str, cut: Option<usize>) {
    for (n, c) in value.chars().enumerate() {
        if cut == Some(n) {
            out.push('\u{2026}');
            break;
        }
        match c {
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

/// What a Value cell shows: escaped, the first 512 characters, then "…".
pub fn display_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len().min(VALUE_CUT * 4) + 3);
    escape_into(&mut out, value, Some(VALUE_CUT));
    out
}

/// True when `value` contains `needle_lower` (already lowered with [`lowercase`]), ignoring
/// case. ASCII values are matched without allocating.
pub fn value_matches(value: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if value.is_ascii() {
        // Lowering an ASCII text gives ASCII, so a needle with other characters never matches.
        if !needle_lower.is_ascii() {
            return false;
        }
        let (h, n) = (value.as_bytes(), needle_lower.as_bytes());
        return h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    lowercase(value).contains(needle_lower)
}

/// Hit `id` through the chunk index `starts` (the first hit index of each chunk).
fn hit_at<'a>(chunks: &'a [Arc<[StringHit]>], starts: &[u32], id: u32) -> Option<&'a StringHit> {
    let c = starts.partition_point(|&s| s <= id).checked_sub(1)?;
    chunks.get(c)?.get((id - starts[c]) as usize)
}

fn encoding_rank(enc: StringEncoding) -> u8 {
    match enc {
        StringEncoding::Ascii => 0,
        StringEncoding::Utf8 => 1,
        StringEncoding::Utf16Le => 2,
    }
}

/// The hit indexes in `sort` order; ties go by address, then hit index. Runs on the background
/// executor.
fn sort_perm(chunks: &[Arc<[StringHit]>], sort: StringsSort) -> Arc<[u32]> {
    let flat: Vec<&StringHit> = chunks.iter().flat_map(|c| c.iter()).collect();
    let mut ids: Vec<u32> = (0..flat.len() as u32).collect();
    ids.sort_unstable_by(|&a, &b| {
        let (x, y) = (flat[a as usize], flat[b as usize]);
        let by = match sort.column {
            StringsColumn::Address => Ordering::Equal,
            StringsColumn::Encoding => encoding_rank(x.encoding).cmp(&encoding_rank(y.encoding)),
            StringsColumn::Length => x.length.cmp(&y.length),
            StringsColumn::Value => x.value.cmp(&y.value),
        };
        let o = by.then(x.address.cmp(&y.address)).then(a.cmp(&b));
        if sort.ascending { o } else { o.reverse() }
    });
    ids.into()
}

/// A view and its inverse: the hit ids shown in order, and per hit id its view position.
type ViewLists = (Arc<[u32]>, Arc<[u32]>);

/// The view (`perm` filtered by `keep`) and its inverse (hit index to view position, `u32::MAX`
/// when hidden), or `None` for all hits in address order. Chunks only grow, so a permutation made
/// before later chunks arrived covers the ids below its length; the newer ids follow it in address
/// order until the new sort lands.
fn compose(perm: Option<Arc<[u32]>>, keep: Option<Arc<[u32]>>, total: u32) -> Option<ViewLists> {
    let perm = perm.map(|p| match (p.len() as u32).cmp(&total) {
        Ordering::Less => p.iter().copied().chain(p.len() as u32..total).collect(),
        Ordering::Equal => p,
        Ordering::Greater => p.iter().copied().filter(|&i| i < total).collect(),
    });
    let view: Arc<[u32]> = match (perm, keep) {
        (None, None) => return None,
        (Some(p), None) => p,
        (None, Some(k)) => k,
        (Some(p), Some(k)) => {
            let mut shown = vec![false; total as usize];
            for &i in k.iter() {
                if let Some(s) = shown.get_mut(i as usize) {
                    *s = true;
                }
            }
            p.iter()
                .copied()
                .filter(|&i| shown.get(i as usize).copied().unwrap_or(false))
                .collect()
        }
    };
    let mut inverse = vec![u32::MAX; total as usize];
    for (pos, &id) in view.iter().enumerate() {
        if let Some(slot) = inverse.get_mut(id as usize) {
            *slot = pos as u32;
        }
    }
    Some((view, inverse.into()))
}

// ---- The table ----

pub struct StringsDelegate {
    chunks: Arc<[Arc<[StringHit]>]>,
    /// The first hit index of each chunk.
    starts: Vec<u32>,
    total: u32,
    /// `None`: every hit in address order.
    view: Option<Arc<[u32]>>,
    inverse: Option<Arc<[u32]>>,
    sort: StringsSort,
    /// False while scanning: header clicks show "Sorting is available after the scan".
    sortable: bool,
    menu_row: Option<u32>,
    version: u64,
    specs: Vec<ColumnSpec<StringsColumn>>,
    layout: ColumnLayout<StringsColumn>,
    words: Rc<StringsWords>,
    tab: WeakEntity<StringsTab>,
}

fn specs(words: &StringsWords, sortable: bool) -> Vec<ColumnSpec<StringsColumn>> {
    StringsColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width, align) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align,
                kind: match align {
                    ColumnAlign::Right => ColumnKind::Numeric,
                    _ => ColumnKind::Text,
                },
                sortable,
                template: None,
                default_width: px(width),
                tooltip: (!sortable).then(|| words.sort_after_scan.clone()),
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

impl StringsDelegate {
    fn new(words: Rc<StringsWords>, tab: WeakEntity<StringsTab>) -> Self {
        let specs = specs(&words, true);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &StringsColumn::ALL,
            &StringsColumn::ALL,
        );
        Self {
            chunks: Arc::from([]),
            starts: Vec::new(),
            total: 0,
            view: None,
            inverse: None,
            sort: StringsSort::default(),
            sortable: true,
            menu_row: None,
            version: 0,
            specs,
            layout,
            words,
            tab,
        }
    }

    /// The chunk list as the payload holds it. The hits are not copied.
    pub fn chunks(&self) -> &Arc<[Arc<[StringHit]>]> {
        &self.chunks
    }

    /// The hit count of the chunk list.
    pub fn total(&self) -> u32 {
        self.total
    }

    /// Hit `id`.
    pub fn hit(&self, id: u32) -> Option<&StringHit> {
        hit_at(&self.chunks, &self.starts, id)
    }

    /// The hit ids shown, in order.
    pub fn view_ids(&self) -> Vec<u32> {
        match &self.view {
            Some(v) => v.to_vec(),
            None => (0..self.total).collect(),
        }
    }

    pub fn sortable(&self) -> bool {
        self.sortable
    }

    /// The Value cell text of row `ix`.
    pub fn value_text(&self, ix: usize) -> String {
        self.hit(self.row_id(ix))
            .map_or_else(String::new, |h| display_value(&h.value))
    }

    fn set_chunks(&mut self, chunks: Arc<[Arc<[StringHit]>]>) {
        self.starts.clear();
        let mut n = 0u32;
        for c in chunks.iter() {
            self.starts.push(n);
            n = n.saturating_add(c.len() as u32);
        }
        self.total = n;
        self.chunks = chunks;
        self.version += 1;
    }

    fn set_sortable(&mut self, sortable: bool) {
        if self.sortable != sortable {
            self.sortable = sortable;
            self.specs = specs(&self.words, sortable);
            self.version += 1;
        }
    }

    /// "0x…\tASCII\t14\tvalue", the value escaped but not cut.
    fn row_line(&self, id: u32) -> Option<String> {
        let h = self.hit(id)?;
        let mut line = format!("0x{:x}\t{}\t", h.address, self.words.encoding(h.encoding));
        units::count(&mut line, u64::from(h.length));
        line.push('\t');
        escape_into(&mut line, &h.value, None);
        Some(line)
    }
}

impl TreeTableDelegate for StringsDelegate {
    type RowId = u32;
    type Column = StringsColumn;
    type Sort = StringsSort;

    fn row_count(&self) -> usize {
        self.view.as_ref().map_or(self.total as usize, |v| v.len())
    }
    fn row_id(&self, ix: usize) -> u32 {
        match &self.view {
            Some(v) => v[ix],
            None => ix as u32,
        }
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        match &self.inverse {
            Some(inv) => inv
                .get(id as usize)
                .filter(|&&p| p != u32::MAX)
                .map(|&p| p as usize),
            None => (id < self.total).then_some(id as usize),
        }
    }
    fn row_kind(&self, _: usize) -> RowKind {
        RowKind::Plain
    }
    fn row_depth(&self, _: usize) -> u8 {
        0
    }
    fn row_expandable(&self, _: usize) -> Option<bool> {
        None
    }
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<StringsColumn> {
        &self.layout
    }
    fn column_spec(&self, id: StringsColumn) -> &ColumnSpec<StringsColumn> {
        let i = StringsColumn::ALL
            .iter()
            .position(|c| *c == id)
            .unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: StringsColumn, out: &mut CellOut) {
        let Some(h) = self.hit(self.row_id(ix)) else {
            return;
        };
        out.validity = Validity::Valid;
        match col {
            StringsColumn::Address => {
                out.bits = h.address;
                if out.want_text {
                    let _ = write!(out.text, "0x{:x}", h.address);
                }
            }
            StringsColumn::Encoding => {
                out.bits = u64::from(encoding_rank(h.encoding));
                if out.want_text {
                    out.text.push_str(self.words.encoding(h.encoding));
                }
            }
            StringsColumn::Length => {
                out.bits = u64::from(h.length);
                if out.want_text {
                    units::count(&mut out.text, u64::from(h.length));
                }
            }
            StringsColumn::Value => {
                out.bits = Arc::as_ptr(&h.value) as *const u8 as u64;
                if out.want_text {
                    escape_into(&mut out.text, &h.value, Some(VALUE_CUT));
                }
            }
        }
    }

    /// The Value cell: escaped and cut, never the raw value.
    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: self.value_text(ix).into(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<StringsColumn> {
        Some(StringsColumn::Value)
    }

    fn header_total(&self, _: StringsColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> StringsSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    fn context_menu(
        &mut self,
        targets: &[u32],
        _: Option<StringsColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let w = self.words.clone();
        self.menu_row = targets.first().copied();
        menu.item(w.copy_value.clone(), Box::new(StringsCopyValue))
            .item(w.copy_address.clone(), Box::new(StringsCopyAddress))
            .item(w.copy_row.clone(), Box::new(StringsCopyRow))
    }

    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    /// Sorting runs on the background executor; the tab applies the result.
    fn on_sort(&mut self, col: StringsColumn, cx: &mut Context<TlTreeTable<Self>>) {
        if !self.sortable {
            return;
        }
        self.sort = StringsSort {
            column: col,
            ascending: self.sort.column != col || !self.sort.ascending,
        };
        self.version += 1;
        cx.notify();
        let tab = self.tab.clone();
        cx.defer(move |cx| {
            let _ = tab.update(cx, |t, cx| t.resort(cx));
        });
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<StringsColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    /// None: a value may hold a secret, and the cell already shows it.
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        (self.total > 0).then(|| self.words.no_match.clone())
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- The status row ----

/// The progress row: the bar while scanning, then the status text. Its own view, so a 4 Hz
/// payload re-renders only this row and the table.
pub struct StringsStatus {
    text: SharedString,
    /// `Some(pct)` while scanning.
    pct: Option<u8>,
}

impl StringsStatus {
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// True while the bar shows.
    pub fn bar_shown(&self) -> bool {
        self.pct.is_some()
    }
}

impl Render for StringsStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let mut text = div()
            .debug_selector(|| "StringsStatus-text".into())
            .text_color(theme.text_secondary)
            .whitespace_nowrap();
        *text.text_style() = numeric_style(&m, m.text.sm);
        h_flex()
            .debug_selector(|| "StringsStatus".into())
            .flex_none()
            .w_full()
            .h(px(STATUS_H * s))
            .px(m.sp[5])
            .gap(m.sp[4])
            .items_center()
            .when_some(self.pct, |d, pct| {
                d.child(
                    div()
                        .debug_selector(|| "StringsProgress".into())
                        .flex_none()
                        .w(px(PROGRESS_W * s))
                        .child(
                            TlProgress::new()
                                .value(f32::from(pct))
                                .size(TlProgressSize::Md)
                                .reduced_motion(true),
                        ),
                )
            })
            .child(text.child(self.text.clone()))
    }
}

// ---- The tab ----

pub struct StringsTab {
    key: ProcessKey,
    table: Entity<TlTreeTable<StringsDelegate>>,
    filter: Entity<PropsFilter>,
    status: Entity<StringsStatus>,
    min_len: Entity<InputState>,
    options_focus: FocusHandle,
    opts: StringScanOpts,
    /// The current scan; payloads of any other are dropped.
    scan: Option<ScanId>,
    /// `None` before the first scan.
    phase: Option<ScanPhase>,
    last: Option<Arc<StringScanState>>,
    found: u32,
    progress_pct: u8,
    /// The sort permutation, `None` for address order.
    perm: Option<Arc<[u32]>>,
    /// The filter matches, `None` when the filter is empty.
    keep: Option<Arc<[u32]>>,
    /// Latest-wins counters of the background sort, view and filter jobs.
    sort_gen: u64,
    view_gen: u64,
    filter_run: u64,
    failed_toast: bool,
    kernel_thread: bool,
    exited: bool,
    /// The body width the Value column was last fitted to.
    flex_body: f32,
    words: Rc<StringsWords>,
    weak: WeakEntity<Self>,
    _subs: SmallVec<[Subscription; 4]>,
}

fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
    }
}

fn send(cmd: Command, cx: &App) {
    match cx.try_global::<EngineGlobal>() {
        Some(engine) => {
            if let Err(err) = engine.0.send(cmd) {
                warn!(target: "tasklens::ui", ?err, "strings command not sent");
            }
        }
        None => warn!(target: "tasklens::ui", "no engine, strings command not sent"),
    }
}

impl StringsTab {
    pub fn new(
        key: ProcessKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(StringsWords::translated(cx));
        let weak = cx.entity().downgrade();
        let delegate = StringsDelegate::new(words.clone(), weak.clone());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let status = cx.new(|_| StringsStatus {
            text: SharedString::default(),
            pct: None,
        });
        let min_len = cx.new(|cx| InputState::new(window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&filter, |this, _, ev: &FilterChanged, cx| {
            this.run_filter(ev.generation, cx);
        }));
        subs.push(cx.subscribe_in(
            &table,
            window,
            |this, _, ev: &TreeTableEvent, window, cx| {
                if matches!(ev, TreeTableEvent::CopiedRows { .. }) {
                    toast(window, cx, ToastKind::Success, this.words.copied.clone());
                }
            },
        ));
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            let version = cx.try_global::<Strings>().map(|s| s.strings_version);
            if this.words.strings_version != version {
                this.set_words(Rc::new(StringsWords::translated(cx)), cx);
            }
        }));
        if let Some(ends) = crate::request_ends::global(cx) {
            subs.push(cx.subscribe(&ends, |this, _, ev: &RequestEvent, cx| {
                this.on_request_end(*ev, cx);
            }));
        }
        // Closing the window drops the tab: a running scan is cancelled.
        cx.on_release(|this, cx| {
            if let (true, Some(scan)) = (this.scan_running(), this.scan) {
                send(Command::CancelStringScan(scan), cx);
            }
        })
        .detach();
        Self {
            key,
            table,
            filter,
            status,
            min_len,
            options_focus: cx.focus_handle(),
            opts: StringScanOpts::default(),
            scan: None,
            phase: None,
            last: None,
            found: 0,
            progress_pct: 0,
            perm: None,
            keep: None,
            sort_gen: 0,
            view_gen: 0,
            filter_run: 0,
            failed_toast: false,
            kernel_thread: false,
            exited: false,
            flex_body: 0.,
            words,
            weak,
            _subs: subs,
        }
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn table(&self) -> &Entity<TlTreeTable<StringsDelegate>> {
        &self.table
    }

    pub fn filter(&self) -> &Entity<PropsFilter> {
        &self.filter
    }

    pub fn status(&self) -> &Entity<StringsStatus> {
        &self.status
    }

    /// The options bar; Enter there starts a scan.
    pub fn options_focus(&self) -> &FocusHandle {
        &self.options_focus
    }

    pub fn words(&self) -> &Rc<StringsWords> {
        &self.words
    }

    pub fn opts(&self) -> StringScanOpts {
        self.opts
    }

    /// Sets the options, as the options bar does.
    pub fn set_opts(&mut self, opts: StringScanOpts, cx: &mut Context<Self>) {
        if self.opts != opts {
            self.opts = opts;
            cx.notify();
        }
    }

    pub fn scan_id(&self) -> Option<ScanId> {
        self.scan
    }

    /// The scan phase as shown; `None` before the first scan.
    pub fn phase(&self) -> Option<&ScanPhase> {
        self.phase.as_ref()
    }

    /// The last payload taken.
    pub fn last_state(&self) -> Option<&Arc<StringScanState>> {
        self.last.as_ref()
    }

    /// A scan runs: Esc cancels it before anything else.
    pub fn scan_running(&self) -> bool {
        self.phase == Some(ScanPhase::Scanning)
    }

    /// Scan can start: no scan runs, the process runs and is no kernel thread, and at least
    /// one encoding is checked.
    pub fn can_scan(&self) -> bool {
        !self.scan_running() && !self.kernel_thread && !self.exited && self.opts.any_encoding()
    }

    /// The tooltip of a disabled Scan button.
    pub fn scan_tooltip(&self) -> Option<SharedString> {
        if self.exited {
            Some(self.words.not_running.clone())
        } else if !self.opts.any_encoding() {
            Some(self.words.no_encoding.clone())
        } else {
            None
        }
    }

    /// The text shown in place of the table, if any.
    pub fn body_text(&self) -> Option<SharedString> {
        if self.kernel_thread {
            return Some(self.words.kernel_thread.clone());
        }
        match &self.phase {
            None => Some(self.words.idle.clone()),
            Some(ScanPhase::Failed(why)) if self.found == 0 => {
                Some(if &**why == SCAN_PTRACE_BLOCKED {
                    self.words.ptrace_blocked.clone()
                } else {
                    self.words.failed.clone()
                })
            }
            Some(_) => None,
        }
    }

    /// The limit banner shows.
    pub fn limit_banner(&self) -> Option<SharedString> {
        (self.phase == Some(ScanPhase::StoppedLimit) && !self.kernel_thread)
            .then(|| self.words.limit.clone())
    }

    /// The status row text.
    pub fn status_text(&self, cx: &App) -> SharedString {
        self.status.read(cx).text.clone()
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Kernel threads have no user memory: options and Scan are disabled.
    pub fn set_kernel_thread(&mut self, kernel_thread: bool, cx: &mut Context<Self>) {
        if self.kernel_thread != kernel_thread {
            self.kernel_thread = kernel_thread;
            cx.notify();
        }
    }

    /// The process exited: Scan is disabled and a running scan ends with "Scan stopped: the
    /// process exited." The engine sends no more payloads once the window froze.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited == exited {
            return;
        }
        if exited && self.scan_running() {
            self.set_phase(ScanPhase::StoppedExited, cx);
            self.update_status(cx);
        }
        self.exited = exited;
        cx.notify();
    }

    fn set_words(&mut self, words: Rc<StringsWords>, cx: &mut Context<Self>) {
        self.words = words.clone();
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.specs = specs(&words, d.sortable);
            d.words = words;
            d.version += 1;
            t.on_new_version(cx);
        });
        self.update_status(cx);
        cx.notify();
    }

    /// Starts a scan with the current options: a new `ScanId`, the previous results cleared.
    /// Returns false when Scan is disabled.
    pub fn start_scan(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.can_scan() {
            return false;
        }
        let scan = ScanId(NEXT_SCAN.fetch_add(1, AtomicOrdering::Relaxed));
        self.scan = Some(scan);
        self.last = None;
        self.found = 0;
        self.progress_pct = 0;
        self.failed_toast = false;
        self.perm = None;
        self.sort_gen += 1;
        self.filter_run += 1;
        let filtering = !self.filter.read(cx).needle().is_empty();
        self.keep = filtering.then(|| Arc::from([]));
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.set_chunks(Arc::from([]));
            d.sort = StringsSort::default();
            d.menu_row = None;
            t.on_new_version(cx);
        });
        self.recompose(cx);
        self.set_phase(ScanPhase::Scanning, cx);
        self.update_status(cx);
        send(
            Command::StringScan {
                scan,
                key: self.key,
                opts: self.opts,
            },
            cx,
        );
        true
    }

    /// Cancels the running scan. The results so far stay. Returns false when no scan runs.
    pub fn cancel_scan(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.scan_running() {
            return false;
        }
        if let Some(scan) = self.scan {
            send(Command::CancelStringScan(scan), cx);
        }
        // At once, so the next Esc goes on to the filter.
        self.set_phase(ScanPhase::Cancelled, cx);
        self.update_status(cx);
        true
    }

    /// The engine ended a request. When it is the running scan and it ended `Lost` or `Busy`, no
    /// payload will say so, so the scan stops here as failed and Scan can start a new one. The
    /// other ends come with the scan's last payload.
    pub fn on_request_end(&mut self, ev: RequestEvent, cx: &mut Context<Self>) {
        if ev.kind != IdKind::Scan
            || self.scan != Some(ScanId(ev.id))
            || !self.scan_running()
            || !matches!(ev.end, RequestEnd::Lost | RequestEnd::Busy)
        {
            return;
        }
        self.set_phase(ScanPhase::Failed(Arc::from(SCAN_LOST)), cx);
        self.update_status(cx);
    }

    /// Takes the tab's payload. A payload of another scan, or the same `Arc` again, does nothing.
    pub fn set_payload(
        &mut self,
        payload: Option<&Arc<TabPayload>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let Some(TabPayload::Strings(state)) = payload.map(|p| &**p) else {
            return;
        };
        if self.scan != Some(state.scan) {
            return;
        }
        if self.last.as_ref().is_some_and(|l| Arc::ptr_eq(l, state)) {
            return;
        }
        // Chunks that arrive after Cancel are kept, but their Scanning phase is not.
        let phase = match (&self.phase, &state.phase) {
            (Some(ScanPhase::Cancelled), ScanPhase::Scanning) => ScanPhase::Cancelled,
            (_, p) => p.clone(),
        };
        self.last = Some(state.clone());
        self.found = state.found;
        self.progress_pct = state.progress_pct.min(100);
        let chunks = state.chunks.clone();
        let (grew, sorted) = self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            let grew = !Arc::ptr_eq(&d.chunks, &chunks);
            d.set_chunks(chunks);
            t.on_new_version(cx);
            (grew, t.delegate.sort != StringsSort::default())
        });
        if self.phase.as_ref() != Some(&phase) {
            self.set_phase(phase.clone(), cx);
        }
        self.update_status(cx);
        if matches!(phase, ScanPhase::Failed(_)) && self.found > 0 && !self.failed_toast {
            self.failed_toast = true;
            toast(window, cx, ToastKind::Warning, self.words.failed.clone());
        }
        // Late chunks (after Cancel) need a new sort and a new filter run. The new sort also
        // makes any sort still running on the old chunks stale.
        if grew && sorted {
            self.resort(cx);
        }
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        } else if grew && self.perm.is_some() {
            self.recompose(cx);
        }
    }

    fn set_phase(&mut self, phase: ScanPhase, cx: &mut Context<Self>) {
        let sortable = phase != ScanPhase::Scanning;
        self.phase = Some(phase);
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_sortable(sortable);
            t.on_new_version(cx);
        });
        cx.notify();
    }

    fn update_status(&mut self, cx: &mut Context<Self>) {
        let Some(phase) = &self.phase else {
            return;
        };
        let text = self.words.status_text(phase, self.progress_pct, self.found);
        let pct = (*phase == ScanPhase::Scanning).then_some(self.progress_pct);
        self.status.update(cx, |s, cx| {
            if s.text != text || s.pct != pct {
                s.text = text;
                s.pct = pct;
                cx.notify();
            }
        });
    }

    /// Sorts on the background executor; address ascending needs no permutation. Only the latest
    /// job may install its permutation.
    fn resort(&mut self, cx: &mut Context<Self>) {
        self.sort_gen += 1;
        let generation = self.sort_gen;
        let (sort, chunks) = {
            let d = &self.table.read(cx).delegate;
            (d.sort, d.chunks.clone())
        };
        if sort == StringsSort::default() {
            self.perm = None;
            self.recompose(cx);
            return;
        }
        let task = cx.background_spawn(async move { sort_perm(&chunks, sort) });
        cx.spawn(async move |this, cx| {
            let perm = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.sort_gen == generation {
                    this.perm = Some(perm);
                    this.recompose(cx);
                }
            });
        })
        .detach();
    }

    fn run_filter(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.filter.read(cx).is_current(generation) {
            return;
        }
        self.filter_run += 1;
        let run = self.filter_run;
        let needle = self.filter.read(cx).needle().to_string();
        if needle.is_empty() {
            self.apply_filter(generation, run, None, cx);
            return;
        }
        let (chunks, starts, total) = {
            let d = &self.table.read(cx).delegate;
            (d.chunks.clone(), d.starts.clone(), d.total)
        };
        let keep = move |i: usize| {
            hit_at(&chunks, &starts, i as u32).is_some_and(|h| value_matches(&h.value, &needle))
        };
        let task = self
            .filter
            .update(cx, |f, cx| f.run_filter_fn(total as usize, keep, cx));
        cx.spawn(async move |this, cx| {
            if let Some((generation, keep)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.apply_filter(generation, run, Some(keep), cx)
                });
            }
        })
        .detach();
    }

    fn apply_filter(
        &mut self,
        generation: u64,
        run: u64,
        keep: Option<Arc<[u32]>>,
        cx: &mut Context<Self>,
    ) {
        if run != self.filter_run || !self.filter.update(cx, |f, _| f.accept(generation)) {
            return;
        }
        self.keep = keep;
        self.recompose(cx);
    }

    /// Builds the view from the permutation and the matches: at once for a small table, else on
    /// the background executor. The previous rows stay until the new view arrives.
    fn recompose(&mut self, cx: &mut Context<Self>) {
        self.view_gen += 1;
        let generation = self.view_gen;
        let (perm, keep) = (self.perm.clone(), self.keep.clone());
        let total = self.table.read(cx).delegate.total;
        if total as usize <= SYNC_MAX_ROWS {
            let view = compose(perm, keep, total);
            self.apply_view(generation, view, cx);
            return;
        }
        let task = cx.background_spawn(async move { compose(perm, keep, total) });
        cx.spawn(async move |this, cx| {
            let view = task.await;
            let _ = this.update(cx, |this, cx| this.apply_view(generation, view, cx));
        })
        .detach();
    }

    fn apply_view(&mut self, generation: u64, view: Option<ViewLists>, cx: &mut Context<Self>) {
        if generation != self.view_gen {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            let (view, inverse) = view.unzip();
            d.view = view;
            d.inverse = inverse;
            d.version += 1;
            t.on_new_version(cx);
        });
    }

    // ---- Copy ----

    fn copy(&self, text: String, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// The selected hit ids in view order.
    fn selected(&self, cx: &App) -> Vec<u32> {
        let t = self.table.read(cx);
        let d = &t.delegate;
        let mut ids: Vec<(usize, u32)> = t
            .selection
            .ids()
            .filter_map(|i| d.row_index(i).map(|ix| (ix, i)))
            .collect();
        ids.sort_unstable();
        ids.into_iter().map(|(_, i)| i).collect()
    }

    /// Ctrl+C: the full values of the selected rows, one per line.
    fn on_copy_rows(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected(cx);
        let text = {
            let d = &self.table.read(cx).delegate;
            ids.iter()
                .filter_map(|&i| d.hit(i).map(|h| h.value.clone()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        if !ids.is_empty() {
            self.copy(text, window, cx);
        }
    }

    fn menu_hit<R>(
        &self,
        cx: &App,
        f: impl FnOnce(&StringsDelegate, u32) -> Option<R>,
    ) -> Option<R> {
        let d = &self.table.read(cx).delegate;
        d.menu_row.and_then(|i| f(d, i))
    }

    /// The full value, not the 512-character cut.
    fn on_copy_value(&mut self, _: &StringsCopyValue, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.menu_hit(cx, |d, i| d.hit(i).map(|h| h.value.to_string())) {
            self.copy(text, window, cx);
        }
    }

    fn on_copy_address(
        &mut self,
        _: &StringsCopyAddress,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.menu_hit(cx, |d, i| d.hit(i).map(|h| format!("0x{:x}", h.address)))
        {
            self.copy(text, window, cx);
        }
    }

    /// The row as TSV: address, encoding, length and the escaped full value.
    fn on_copy_row(&mut self, _: &StringsCopyRow, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.menu_hit(cx, |d, i| d.row_line(i)) {
            self.copy(text, window, cx);
        }
    }

    // ---- Render ----

    fn options_bar(&self, cx: &App) -> impl IntoElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let w = &self.words;
        let scanning = self.scan_running();
        let locked = scanning || self.kernel_thread || self.exited;
        let mut label = div().text_color(theme.text_secondary).whitespace_nowrap();
        *label.text_style() = t_sm(&m);

        let weak = self.weak.clone();
        let stepper = TlStepper::new("StringsMinLength", &self.min_len)
            .range(
                i64::from(*StringScanOpts::MIN_LEN_RANGE.start()),
                i64::from(*StringScanOpts::MIN_LEN_RANGE.end()),
                1,
            )
            .value(i64::from(self.opts.min_len))
            .disabled(locked)
            .on_change(move |v, _, cx| {
                let _ = weak.update(cx, |t, cx| {
                    let mut opts = t.opts;
                    opts.min_len =
                        u8::try_from(v).unwrap_or(*StringScanOpts::MIN_LEN_RANGE.start());
                    t.set_opts(opts, cx);
                });
            });

        let check = |id: &'static str,
                     label: &SharedString,
                     on: bool,
                     set: fn(&mut StringScanOpts, bool)| {
            let weak = self.weak.clone();
            TlCheckbox::new(id)
                .checked(Some(on))
                .label(label.clone())
                .disabled(locked)
                .on_change(move |checked, _, cx| {
                    let _ = weak.update(cx, |t, cx| {
                        let mut opts = t.opts;
                        set(&mut opts, checked);
                        t.set_opts(opts, cx);
                    });
                })
        };

        let button = if scanning {
            TlButton::new("StringsCancel", TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::Square)
                .icon_size(px(CANCEL_ICON * s))
                .label(w.cancel.clone())
                .tooltip(w.cancel_tip.clone())
                .action(Box::new(PropsCancelStringScan))
        } else {
            TlButton::new("StringsScan", TlButtonKind::Primary)
                .size(TlButtonSize::Sm)
                .icon(TlIcon::ScanSearch)
                .icon_size(px(SCAN_ICON * s))
                .label(w.scan.clone())
                .disabled(!self.can_scan())
                .when_some(self.scan_tooltip(), |b, tip| b.tooltip(tip))
                .action(Box::new(PropsStartStringScan))
        };

        h_flex()
            .id("StringsOptions")
            .debug_selector(|| "StringsOptions".into())
            .key_context(STRINGS_OPTIONS_CONTEXT)
            .track_focus(&self.options_focus)
            .flex_none()
            .w_full()
            .h(px(OPTIONS_H * s))
            .px(m.sp[5])
            .gap(m.sp[4])
            .items_center()
            .border_b_1()
            .border_color(theme.divider)
            .child(label.child(w.min_length.clone()))
            .child(stepper)
            .child(check(
                "StringsAscii",
                &w.encodings[0],
                self.opts.ascii,
                |o, v| o.ascii = v,
            ))
            .child(check(
                "StringsUtf8",
                &w.encodings[1],
                self.opts.utf8,
                |o, v| o.utf8 = v,
            ))
            .child(check(
                "StringsUtf16le",
                &w.encodings[2],
                self.opts.utf16le,
                |o, v| o.utf16le = v,
            ))
            .child(check(
                "StringsFileBacked",
                &w.include_file_backed,
                self.opts.include_file_backed,
                |o, v| o.include_file_backed = v,
            ))
            .child(div().flex_1())
            .child(button)
    }

    fn limit_banner_element(&self, text: SharedString, cx: &App) -> impl IntoElement {
        let theme = current(cx);
        let m = *metrics(cx);
        let s = m.ui_scale;
        let warning = theme.status.warning;
        let mut label = div()
            .debug_selector(|| "StringsLimitBanner-text".into())
            .text_color(theme.text_primary);
        *label.text_style() = t_sm(&m);
        h_flex()
            .debug_selector(|| "StringsLimitBanner".into())
            .flex_none()
            .w_full()
            .h(px(STATUS_H * s))
            .px(m.sp[5])
            .gap(m.sp[3])
            .items_center()
            .bg(warning.opacity(BANNER_FILL_ALPHA))
            .child(icon(TlIcon::TriangleAlert, px(BANNER_ICON * s), warning))
            .child(label.child(text))
    }
}

impl Render for StringsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The Value column is the flex column: it takes the width the others leave.
        let body = window.viewport_size().width.as_f32();
        if self.flex_body != body {
            self.flex_body = body;
            self.table.update(cx, |t, cx| {
                if fit_flex_column(t.delegate_mut(), body) {
                    t.on_new_version(cx);
                }
            });
        }
        let m = *metrics(cx);
        let theme = current(cx).clone();
        let root = v_flex()
            .debug_selector(|| "PropsStrings".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_rows))
            .on_action(cx.listener(Self::on_copy_value))
            .on_action(cx.listener(Self::on_copy_address))
            .on_action(cx.listener(Self::on_copy_row))
            .child(self.options_bar(cx));
        if let Some(text) = self.body_text() {
            let mut label = div()
                .debug_selector(|| "StringsBody-text".into())
                .text_center()
                .text_color(theme.text_secondary);
            *label.text_style() = t_base(&m);
            return root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(label.child(text)),
            );
        }
        let banner = self
            .limit_banner()
            .map(|t| self.limit_banner_element(t, cx));
        root.child(self.status.clone())
            .children(banner)
            .child(div().flex_1().min_h_0().w_full().child(self.table.clone()))
    }
}

/// Gives the Value column the width the other columns leave in a body `body_px` wide (logical
/// px at the layout's scale), never below its minimum. Returns true when the width changed.
fn fit_flex_column(d: &mut StringsDelegate, body_px: f32) -> bool {
    let scale = d.layout.ui_scale().max(0.1);
    let flex = StringsColumn::Value;
    let others: f32 = StringsColumn::ALL
        .iter()
        .filter(|&&c| c != flex)
        .map(|c| d.layout.widths.get(c).map_or(0.0, |w| w.as_f32()))
        .sum();
    let min = flex.min_width().unwrap_or(0.);
    let target = (body_px / scale - others).max(min);
    let current = d.layout.widths.get(&flex).map_or(0.0, |w| w.as_f32());
    if (target - current).abs() < 0.5 {
        return false;
    }
    d.layout.widths.insert(flex, px(target));
    d.layout.rebuild(scale);
    d.version += 1;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_progress_text_format() {
        let w = StringsWords::en();
        assert_eq!(
            w.status_text(&ScanPhase::Scanning, 42, 18_234),
            "Scanning process memory... 42%  18\u{2009}234 strings found"
        );
        assert_eq!(
            w.status_text(&ScanPhase::Finished, 100, 18_234),
            "Scan finished. 18\u{2009}234 strings found."
        );
        assert_eq!(
            w.status_text(&ScanPhase::Cancelled, 7, 18_234),
            "Scan cancelled. 18\u{2009}234 strings found."
        );
        assert_eq!(
            w.status_text(&ScanPhase::StoppedExited, 7, 18_234),
            "Scan stopped: the process exited. 18\u{2009}234 strings found."
        );
    }

    #[test]
    fn strings_compose_keeps_hits_newer_than_the_permutation() {
        // A permutation of 3 hits after the chunks grew to 5: ids 3 and 4 follow it.
        let perm: Arc<[u32]> = Arc::from(vec![2, 0, 1]);
        let (view, inverse) = compose(Some(perm.clone()), None, 5).expect("a view");
        assert_eq!(&*view, &[2, 0, 1, 3, 4]);
        assert_eq!(&*inverse, &[1, 2, 0, 3, 4]);
        let keep: Arc<[u32]> = Arc::from(vec![0, 3]);
        let (view, _) = compose(Some(perm), Some(keep), 5).expect("a view");
        assert_eq!(&*view, &[0, 3]);
    }

    #[test]
    fn strings_display_value_escapes_and_cuts() {
        assert_eq!(display_value("a\tb\nc\rd"), "a\\tb\\nc\\rd");
        let long: String = "x".repeat(600);
        let shown = display_value(&long);
        assert_eq!(shown.chars().count(), VALUE_CUT + 1);
        assert!(shown.ends_with('\u{2026}'));
        assert_eq!(
            display_value(&"y".repeat(512)),
            "y".repeat(512),
            "512 fits, no ellipsis"
        );
    }

    #[test]
    fn strings_value_matches_ignores_case() {
        assert!(value_matches("/usr/LIB/firefox", "lib"));
        assert!(!value_matches("/usr/lib", "bin"));
        assert!(value_matches("ÄBC", &lowercase("äb")));
        assert!(!value_matches("abc", "ä"));
        assert!(value_matches("anything", ""));
    }

    #[test]
    fn strings_chunk_index_finds_rows() {
        let hit = |a: u64| StringHit {
            address: a,
            encoding: StringEncoding::Ascii,
            length: 4,
            value: Arc::from("text"),
        };
        let chunks: Vec<Arc<[StringHit]>> = vec![
            Arc::from(vec![hit(1), hit(2)]),
            Arc::from(Vec::<StringHit>::new()),
            Arc::from(vec![hit(3)]),
        ];
        let starts = [0, 2, 2];
        assert_eq!(hit_at(&chunks, &starts, 0).map(|h| h.address), Some(1));
        assert_eq!(hit_at(&chunks, &starts, 1).map(|h| h.address), Some(2));
        assert_eq!(hit_at(&chunks, &starts, 2).map(|h| h.address), Some(3));
        assert!(hit_at(&chunks, &starts, 3).is_none());
    }

    #[test]
    fn strings_compose_sort_then_filter() {
        let perm: Arc<[u32]> = Arc::from(vec![2, 0, 1]);
        let keep: Arc<[u32]> = Arc::from(vec![0, 2]);
        let (view, inverse) = compose(Some(perm), Some(keep), 3).expect("a view");
        assert_eq!(&*view, &[2, 0]);
        assert_eq!(&*inverse, &[1, u32::MAX, 0]);
        assert!(compose(None, None, 3).is_none());
    }
}
