//! The Search File Handles window "tasklens Handle Search": pattern, include-maps option,
//! Search and Stop, progress, the footer note and "Search all" through the helper
//! (UI-PWN-030, UI-SHL-098, PROC-096, APP-015, R-061, R-102, R-119), and the result table with
//! its row menu (UI-PWN-031): End Task, End Process, Go to process, Copy path and Open file
//! location.
//!
//! Security: "Search all" only dispatches `StartHelperSession` and marks this window as the
//! start's requester, so the helper flow toasts a failed start here; the engine elevates. End Task
//! and End Process follow the Processes page rules (PROC-059 to PROC-061, R-036, D-009): one own
//! unprotected process is sent at once, two or more processes, a protected process or another
//! user's process ask first, and tasklens itself, PID 1 and kernel threads are refused. The
//! pattern and the paths are never logged (ADR-015).
//!
//! The words are `en-US.ftl` keys, translated once per strings version into
//! [`HandleSearchWords`].

use std::cmp::Ordering as Cmp;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::component::input::{Enter, Escape, InputEvent, InputState};
use gpui_kit::{
    AnyView, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest, ActionTarget, RejectReason, Signal};
use tl_model::command::{Command, Tab};
use tl_model::details::{HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId};
use tl_model::health::{HelperStartOutcome, HelperState};
use tl_model::ids::{NodeId, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessRecord};
use tl_model::snapshot::{ComponentMask, ProcessSnapshot, Snapshot};
use tl_model::units;
use tl_model::validity::Validity;
use tracing::warn;

use super::actions::{
    ClearHandleSearch, HANDLE_SEARCH_CONTEXT, HandleSearchAll, HandleSearchCopyPaths,
    HandleSearchCopyRows, HandleSearchEndProcess, HandleSearchEndTask, HandleSearchGoToProcess,
    StartHandleSearch, StopHandleSearch,
};
use super::frame::{PwnWindow, TITLE_HANDLE_SEARCH, pwn_window_spec};
use crate::action_tracker::{GlobalActionTracker, ToastTexts, action_words};
use crate::actions::OpenHandleSearch;
use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::checkbox::TlCheckbox;
use crate::components::input::{TlInputSize, TlTextInput};
use crate::components::menu::TlMenu;
use crate::components::progress::{TlProgress, TlProgressSize};
use crate::components::shield::StartHelperSession;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::components::window_frame::{CloseSecondaryWindow, TlSecondaryWindow};
use crate::hub::{GlobalHub, SnapshotChanged};
use crate::i18n::{Strings, tr};
use crate::pages::performance::details::Template;
use crate::pages::processes::ProcessesPage;
use crate::pages::processes::actions::{CapturedTarget, CapturedTargets};
use crate::pages::processes::confirm::{ConfirmKind, ConfirmResult, ProtectedAction, confirm};
use crate::pages::processes::jump::JumpTexts;
use crate::pages::processes::toasts::{refusal_reason, toast_words};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate,
};
use crate::text::style::{numeric_style, t_base, t_sm};
use crate::theme::{current, metrics};
use crate::window::main::{ShellState, main_window_view};
use crate::window::registry::{GlobalRegistry, WindowKey};

gpui_kit::actions!(tasklens_handles, [HandleSearchOpenLocation]);

/// The most hits the window shows. At this count the search stops and the status reads
/// "Showing the first 100 000 matches.".
pub const HANDLE_HIT_CAP: usize = 100_000;

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
    /// Header labels in [`HitColumn::ALL`] order.
    pub headers: [SharedString; 6],
    pub end_task: SharedString,
    pub end_process: SharedString,
    pub go_to: SharedString,
    pub copy_path: SharedString,
    pub open_location: SharedString,
    pub copied: SharedString,
    pub opening: SharedString,
    pub open_failed: SharedString,
    retrieving: Template,
    found: Template,
    stopped: Template,
    footer: Template,
    capped: Template,
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
            headers: HitColumn::ALL.map(|c| tr(cx, c.meta().0)),
            end_task: tr(cx, "pwn-handle-search-menu-end-task"),
            end_process: tr(cx, "pwn-handle-search-menu-end-process"),
            go_to: tr(cx, "pwn-handle-search-menu-go-to"),
            copy_path: tr(cx, "pwn-handle-search-menu-copy-path"),
            open_location: tr(cx, "pwn-handle-search-menu-open-location"),
            copied: tr(cx, "pwn-handle-search-toast-copied"),
            opening: tr(cx, "pwn-handle-search-toast-opening"),
            open_failed: tr(cx, "pwn-handle-search-toast-open-failed"),
            retrieving: Template::new(cx, "pwn-handle-search-retrieving", &["pct", "n"]),
            found: Template::counted(cx, "pwn-handle-search-found", &["n"], 0),
            stopped: Template::counted(cx, "pwn-handle-search-stopped", &["n"], 0),
            footer: Template::new(
                cx,
                "pwn-handle-search-footer",
                &["searched", "total", "skipped"],
            ),
            capped: Template::new(cx, "pwn-handle-search-capped", &["n"]),
        }
    }

    /// "Showing the first 100 000 matches.".
    pub fn capped_text(&self) -> SharedString {
        let mut n = String::new();
        units::count(&mut n, HANDLE_HIT_CAP as u64);
        let mut out = String::new();
        let _ = self.capped.write(&mut out, &[&n]);
        out.into()
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

// ---- The result table ----

/// The columns of the result table, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HitColumn {
    Proc,
    Pid,
    User,
    Type,
    Fd,
    Path,
}

impl HitColumn {
    pub const ALL: [HitColumn; 6] = [
        Self::Proc,
        Self::Pid,
        Self::User,
        Self::Type,
        Self::Fd,
        Self::Path,
    ];

    /// Header key, width and minimum at 100%, alignment. Path is the flex column: its width is
    /// what the others leave.
    fn meta(self) -> (&'static str, f32, f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Proc => ("pwn-handle-search-col-process", 200., 100., Left),
            Self::Pid => ("pwn-handle-search-col-pid", 80., 60., Right),
            Self::User => ("pwn-handle-search-col-user", 110., 60., Left),
            Self::Type => ("pwn-handle-search-col-type", 70., 60., Left),
            Self::Fd => ("pwn-handle-search-col-fd", 60., 60., Right),
            Self::Path => ("pwn-handle-search-col-path", 200., 200., Left),
        }
    }
}

impl TableColumn for HitColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Proc => "proc",
            Self::Pid => "pid",
            Self::User => "user",
            Self::Type => "type",
            Self::Fd => "fd",
            Self::Path => "path",
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
        Some(self.meta().2)
    }
}

/// The sort of the result table, kept for the window's life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HitSort {
    pub column: HitColumn,
    pub ascending: bool,
}

impl Default for HitSort {
    /// Process ascending, then PID, then FD.
    fn default() -> Self {
        Self {
            column: HitColumn::Proc,
            ascending: true,
        }
    }
}

impl TableSort<HitColumn> for HitSort {
    fn column(&self) -> HitColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The Type cell: `fd`, `mmap`, `cwd`, `exe` or `root`.
pub fn kind_text(kind: HandleKind) -> &'static str {
    match kind {
        HandleKind::Fd => "fd",
        HandleKind::Mmap => "mmap",
        HandleKind::Cwd => "cwd",
        HandleKind::Exe => "exe",
        HandleKind::Root => "root",
    }
}

/// FD order with no FD last.
fn fd_order(fd: Option<i32>) -> (bool, i32) {
    (fd.is_none(), fd.unwrap_or(0))
}

/// Orders hits `a` and `b` by `col`, then by process name, PID and FD.
fn compare(a: &HandleHit, b: &HandleHit, col: HitColumn) -> Cmp {
    let by = match col {
        HitColumn::Proc => Cmp::Equal,
        HitColumn::Pid => a.key.pid.cmp(&b.key.pid),
        HitColumn::User => a.user.cmp(&b.user),
        HitColumn::Type => kind_text(a.kind).cmp(kind_text(b.kind)),
        HitColumn::Fd => fd_order(a.fd).cmp(&fd_order(b.fd)),
        HitColumn::Path => a.path.cmp(&b.path),
    };
    by.then_with(|| a.name.cmp(&b.name))
        .then(a.key.pid.cmp(&b.key.pid))
        .then(fd_order(a.fd).cmp(&fd_order(b.fd)))
}

/// The display text of `col` for hit `h`. The FD cell is empty for other types.
fn write_text(h: &HandleHit, col: HitColumn, out: &mut String) {
    match col {
        HitColumn::Proc => out.push_str(&h.name),
        HitColumn::Pid => {
            let _ = write!(out, "{}", h.key.pid);
        }
        HitColumn::User => out.push_str(&h.user),
        HitColumn::Type => out.push_str(kind_text(h.kind)),
        HitColumn::Fd => {
            if let (HandleKind::Fd, Some(fd)) = (h.kind, h.fd) {
                let _ = write!(out, "{fd}");
            }
        }
        HitColumn::Path => out.push_str(&h.path),
    }
}

fn specs(words: &HandleSearchWords) -> Vec<ColumnSpec<HitColumn>> {
    HitColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (_, width, _, align) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align,
                kind: match (id, align) {
                    (HitColumn::Path, _) => ColumnKind::Path,
                    (_, ColumnAlign::Left) => ColumnKind::Text,
                    (_, ColumnAlign::Right) => ColumnKind::Numeric,
                },
                sortable: true,
                template: None,
                default_width: px(width),
                tooltip: None,
                availability: Availability::Available,
                hideable: false,
            }
        })
        .collect()
}

/// The result table's rows, columns and sort. A row id is the hit's index in the search's
/// chunks; rows read the hits through the chunk index and never copy them.
pub struct HandleHitsDelegate {
    chunks: Arc<[Arc<[HandleHit]>]>,
    /// The hit index each chunk starts at.
    starts: Vec<u32>,
    /// Hits shown: at most [`HANDLE_HIT_CAP`].
    len: usize,
    /// View row to hit index.
    order: Vec<u32>,
    /// Hit index to view row.
    pos: Vec<u32>,
    /// The live processes, for the icons.
    procs: Arc<ProcessSnapshot>,
    version: u64,
    sort: HitSort,
    specs: Vec<ColumnSpec<HitColumn>>,
    layout: ColumnLayout<HitColumn>,
    words: Rc<HandleSearchWords>,
}

impl HandleHitsDelegate {
    fn new(words: Rc<HandleSearchWords>, procs: Arc<ProcessSnapshot>) -> Self {
        let specs = specs(&words);
        let layout =
            ColumnLayout::defaults(TableId::Processes, &specs, &HitColumn::ALL, &HitColumn::ALL);
        Self {
            chunks: Arc::from([]),
            starts: Vec::new(),
            len: 0,
            order: Vec::new(),
            pos: Vec::new(),
            procs,
            version: 0,
            sort: HitSort::default(),
            specs,
            layout,
            words,
        }
    }

    /// The hit `id`, while it is shown.
    pub fn hit(&self, id: u32) -> Option<&HandleHit> {
        let i = id as usize;
        if i >= self.len {
            return None;
        }
        let c = self
            .starts
            .partition_point(|&s| s as usize <= i)
            .checked_sub(1)?;
        self.chunks.get(c)?.get(i - self.starts[c] as usize)
    }

    /// The hit ids shown, in view order.
    pub fn view_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.order.iter().copied()
    }

    /// The text the `col` cell of hit `id` shows.
    pub fn cell_text(&self, id: u32, col: HitColumn) -> Option<String> {
        let h = self.hit(id)?;
        let mut out = String::new();
        write_text(h, col, &mut out);
        Some(out)
    }

    /// Takes the search's chunks. The chunks only grow, so the same count is the same rows and
    /// does not sort again. True when the rows changed.
    fn set_hits(&mut self, chunks: Arc<[Arc<[HandleHit]>]>) -> bool {
        self.starts.clear();
        let mut n = 0usize;
        for c in chunks.iter() {
            self.starts.push(n.min(u32::MAX as usize) as u32);
            n += c.len();
        }
        self.chunks = chunks;
        let len = n.min(HANDLE_HIT_CAP);
        if len == self.len {
            return false;
        }
        self.len = len;
        self.resort();
        true
    }

    /// Drops the rows. True when there were any.
    fn clear(&mut self) -> bool {
        let had = self.len > 0;
        self.chunks = Arc::from([]);
        self.starts.clear();
        self.len = 0;
        self.resort();
        had
    }

    fn resort(&mut self) {
        let hits: Vec<&HandleHit> = self
            .chunks
            .iter()
            .flat_map(|c| c.iter())
            .take(self.len)
            .collect();
        let HitSort { column, ascending } = self.sort;
        self.order.clear();
        self.order.extend(0..hits.len() as u32);
        self.order.sort_unstable_by(|&a, &b| {
            let o = compare(hits[a as usize], hits[b as usize], column);
            if ascending { o } else { o.reverse() }
        });
        self.pos.clear();
        self.pos.resize(hits.len(), 0);
        for (row, &i) in self.order.iter().enumerate() {
            self.pos[i as usize] = row as u32;
        }
        self.version += 1;
    }

    /// `ids` that are shown, in view order.
    pub fn in_view_order(&self, ids: impl IntoIterator<Item = u32>) -> Vec<u32> {
        let mut ids: Vec<u32> = ids
            .into_iter()
            .filter(|&id| (id as usize) < self.len)
            .collect();
        ids.sort_unstable_by_key(|&id| self.pos[id as usize]);
        ids
    }

    /// The first hit of each distinct process among `ids`, in the order of `ids`.
    pub fn distinct_hits(&self, ids: &[u32]) -> Vec<&HandleHit> {
        let mut seen = FxHashSet::default();
        ids.iter()
            .filter_map(|&id| self.hit(id))
            .filter(|h| seen.insert(h.key))
            .collect()
    }

    /// The paths of `ids`, one per line.
    pub fn paths_text(&self, ids: &[u32]) -> String {
        let mut out = String::new();
        for h in ids.iter().filter_map(|&id| self.hit(id)) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&h.path);
        }
        out
    }

    /// `ids` as tab-separated lines, with the header line first when `header`.
    pub fn rows_tsv(&self, ids: &[u32], header: bool) -> String {
        let mut out = String::new();
        if header {
            out.push_str(&self.words.headers.join("\t"));
        }
        for h in ids.iter().filter_map(|&id| self.hit(id)) {
            if !out.is_empty() {
                out.push('\n');
            }
            for (i, &col) in HitColumn::ALL.iter().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                write_text(h, col, &mut out);
            }
        }
        out
    }

    /// Gives the Path column the width the other columns leave in a body `body_px` wide
    /// (logical px at the layout's scale), never below its minimum. True when it changed.
    fn fit_flex_column(&mut self, body_px: f32) -> bool {
        let scale = self.layout.ui_scale().max(0.1);
        let flex = HitColumn::Path;
        let others: f32 = HitColumn::ALL
            .iter()
            .filter(|&&c| c != flex)
            .map(|c| self.layout.widths.get(c).map_or(0.0, |w| w.as_f32()))
            .sum();
        let min = flex.meta().2;
        let target = (body_px / scale - others).max(min);
        let current = self.layout.widths.get(&flex).map_or(0.0, |w| w.as_f32());
        if (target - current).abs() < 0.5 {
            return false;
        }
        self.layout.widths.insert(flex, px(target));
        self.layout.rebuild(scale);
        self.version += 1;
        true
    }

    fn row(&self, ix: usize) -> Option<&HandleHit> {
        self.hit(*self.order.get(ix)?)
    }
}

impl TreeTableDelegate for HandleHitsDelegate {
    type RowId = u32;
    type Column = HitColumn;
    type Sort = HitSort;

    fn row_count(&self) -> usize {
        self.order.len()
    }
    fn row_id(&self, ix: usize) -> u32 {
        self.order[ix]
    }
    fn row_index(&self, id: u32) -> Option<usize> {
        self.pos.get(id as usize).map(|&p| p as usize)
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
    fn columns(&self) -> &ColumnLayout<HitColumn> {
        &self.layout
    }
    fn column_spec(&self, id: HitColumn) -> &ColumnSpec<HitColumn> {
        let i = HitColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: HitColumn, out: &mut CellOut) {
        let Some(h) = self.row(ix) else {
            return;
        };
        let text_bits = |s: &str| (s.as_ptr() as u64) ^ ((s.len() as u64) << 48);
        out.bits = match col {
            HitColumn::Proc => text_bits(&h.name),
            HitColumn::Pid => u64::from(h.key.pid),
            HitColumn::User => text_bits(&h.user),
            HitColumn::Type => h.kind as u64,
            HitColumn::Fd => h.fd.map_or(u64::MAX, |fd| fd as u32 as u64),
            HitColumn::Path => text_bits(&h.path),
        };
        out.validity = Validity::Valid;
        if out.want_text {
            write_text(h, col, &mut out.text);
        }
    }

    /// The process icon from the live record, then the display name.
    fn name_cell(&self, ix: usize) -> NameCell {
        let hit = self.row(ix);
        let icon = hit
            .and_then(|h| self.procs.get(h.key))
            .and_then(|r| r.meta.icon.as_ref())
            .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i)));
        NameCell {
            icon,
            icon_size: IconSlot::Small,
            primary: hit.map_or_else(SharedString::default, |h| SharedString::from(&h.name)),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<HitColumn> {
        Some(HitColumn::Proc)
    }

    fn header_total(&self, _: HitColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> HitSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    /// End Task, End Process, Go to process, Copy path and Open file location (R-102). Open
    /// file location needs one row with an absolute path.
    fn context_menu(
        &mut self,
        targets: &[u32],
        _: Option<HitColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let w = self.words.clone();
        let menu = menu
            .item(w.end_task.clone(), Box::new(HandleSearchEndTask))
            .item(w.end_process.clone(), Box::new(HandleSearchEndProcess))
            .separator()
            .item(w.go_to.clone(), Box::new(HandleSearchGoToProcess))
            .separator()
            .item(w.copy_path.clone(), Box::new(HandleSearchCopyPaths));
        let one_path = match targets {
            [id] => self.hit(*id).is_some_and(|h| h.path.starts_with('/')),
            _ => false,
        };
        if one_path {
            menu.item(w.open_location.clone(), Box::new(HandleSearchOpenLocation))
        } else {
            menu.item_disabled(w.open_location.clone(), None)
        }
    }

    fn on_toggle_expand(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    /// A new column sorts ascending first; the same column flips.
    fn on_sort(&mut self, col: HitColumn, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = HitSort {
            column: col,
            ascending: self.sort.column != col || !self.sort.ascending,
        };
        self.resort();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<HitColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u32], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: u32, _: &mut Context<TlTreeTable<Self>>) {}

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    /// The status line says "No results found.", so the empty table draws nothing.
    fn empty_text(&self) -> Option<SharedString> {
        None
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- Ending processes ----

/// The target of one live process record. A copy of the Processes page's `record_target`.
fn record_target(r: &ProcessRecord) -> CapturedTarget {
    CapturedTarget {
        node: NodeId::Process(r.key),
        kind: RowKind::Process,
        name: SharedString::from(&r.meta.name),
        pid: Some(r.key.pid),
        uid: r.uid,
        user: SharedString::from(&r.meta.user),
        flags: r.flags,
        member_count: 1,
        stopped: r.state == ProcState::Stopped,
        ppid: r.ppid,
        nice: Some(r.nice),
        exe_deleted: false,
        unit: None,
    }
}

/// The target of a hit whose process is no longer listed. It is still sent: the engine checks
/// the key and answers with the identity toast. The uid is not known.
fn hit_target(h: &HandleHit) -> CapturedTarget {
    CapturedTarget {
        node: NodeId::Process(h.key),
        kind: RowKind::Process,
        name: SharedString::from(&h.name),
        pid: Some(h.key.pid),
        uid: u32::MAX,
        user: SharedString::from(&h.user),
        flags: ProcFlags::empty(),
        member_count: 1,
        stopped: false,
        ppid: 0,
        nice: None,
        exe_deleted: false,
        unit: None,
    }
}

/// The confirmation an end action on `targets` needs first, if any (R-036, D-009): a protected
/// target, then two or more targets, then another user's target.
fn confirm_kind(targets: &[CapturedTarget], kill: bool, cx: &App) -> Option<ConfirmKind> {
    if let Some(t) = targets
        .iter()
        .find(|t| t.flags.contains(ProcFlags::PROTECTED))
    {
        return Some(ConfirmKind::Protected {
            action: if kill {
                ProtectedAction::Kill
            } else {
                ProtectedAction::End
            },
            name: t.name.clone(),
            tree_count: None,
        });
    }
    if targets.len() >= 2 {
        return Some(ConfirmKind::EndMany {
            count: targets.len() as u32,
            kill,
        });
    }
    let t = targets
        .iter()
        .find(|t| t.flags.contains(ProcFlags::OTHER_USER))?;
    let action = if kill {
        "processes-confirm-action-end-process"
    } else {
        "processes-confirm-action-end-task"
    };
    Some(ConfirmKind::OtherUser {
        action: tr(cx, action),
        name: t.name.clone(),
        pid: t.pid.unwrap_or_default(),
        user: t.user.clone(),
        destructive: true,
    })
}

/// Sends `request` through the app's `ActionTracker`; its toasts go to `queue`.
fn submit(
    request: ActionRequest,
    queue: WeakEntity<ToastQueue>,
    texts: ToastTexts,
    cx: &mut App,
) -> Option<ActionId> {
    let Some(tracker) = cx.try_global::<GlobalActionTracker>().map(|t| t.0.clone()) else {
        warn!(target: "tasklens::ui", "no action tracker; the handle search action was not sent");
        return None;
    };
    tracker.update(cx, |t, cx| t.track_or_toast(request, queue, texts, cx))
}

/// Selects `key` on the Processes tab and brings the main window to the front.
fn go_to_process(key: ProcessKey, cx: &mut App) {
    let main = cx.try_global::<ShellState>().and_then(|s| s.main_window);
    let page =
        main_window_view(cx).and_then(|v| v.read(cx).page_entity::<ProcessesPage>(Tab::Processes));
    let (Some(main), Some(page)) = (main, page) else {
        warn!(target: "tasklens::ui", "no Processes page for Go to process");
        return;
    };
    let _ = main.update(cx, |_, window, cx| {
        window.activate_window();
        page.update(cx, |p, cx| {
            p.select_by_key(key, JumpTexts::default(), window, cx)
        });
    });
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
    table: Entity<TlTreeTable<HandleHitsDelegate>>,
    /// The live processes: the end actions' targets and the icons.
    procs: Arc<ProcessSnapshot>,
    /// The body width the Path column was last fitted to.
    flex_body: f32,
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
                let words = this.words.clone();
                this.table.update(cx, |t, cx| {
                    let d = t.delegate_mut();
                    d.specs = specs(&words);
                    d.words = words;
                    d.version += 1;
                    t.on_new_version(cx);
                });
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
        let procs = hub
            .as_ref()
            .map(|hub| hub.read(cx).current().processes.clone())
            .unwrap_or_default();
        let delegate = HandleHitsDelegate::new(words.clone(), procs.clone());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
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
            table,
            procs,
            flex_body: 0.,
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

    /// The result table.
    pub fn table(&self) -> &Entity<TlTreeTable<HandleHitsDelegate>> {
        &self.table
    }

    fn pattern_text(&self, cx: &App) -> String {
        self.pattern.read(cx).text().to_string().trim().to_owned()
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
            SearchStatus::Done | SearchStatus::Stopped if self.hits >= HANDLE_HIT_CAP => {
                w.capped_text()
            }
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
        self.clear_table(cx);
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
        self.clear_table(cx);
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
            crate::helper_flow::note_start(window.window_handle(), cx);
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
            self.take_health(snap, cx);
        }
        if mask.intersects(ComponentMask::PROCESSES) {
            self.procs = snap.processes.clone();
            let procs = self.procs.clone();
            self.table.update(cx, |t, _| t.delegate_mut().procs = procs);
        }
        if mask.intersects(ComponentMask::DETAILS) {
            self.take_search(snap.details.handle_search.as_ref(), window, cx);
        }
    }

    fn clear_table(&mut self, cx: &mut Context<Self>) {
        // Row ids are hit indexes, so a new search must not keep the old selection.
        self.table.update(cx, |t, cx| {
            t.selection.clear();
            if t.delegate_mut().clear() {
                t.on_new_version(cx);
            }
        });
    }

    /// A cancel ends "Search all"; the helper flow toasts it.
    fn take_health(&mut self, snap: &Snapshot, cx: &mut Context<Self>) {
        let health = &snap.health;
        if let Some((seq, outcome)) = &health.helper_start
            && self.start_seq != Some(*seq)
        {
            self.start_seq = Some(*seq);
            if self.pending_all && *outcome == HelperStartOutcome::Cancelled {
                self.pending_all = false;
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
        let chunks = payload.chunks.clone();
        self.table.update(cx, |t, cx| {
            if t.delegate_mut().set_hits(chunks) {
                t.on_new_version(cx);
            }
        });
        // At the cap the search stops and the status says so.
        if self.state == SearchStatus::Searching && self.hits >= HANDLE_HIT_CAP {
            if let Some(id) = self.search {
                send(Command::CancelHandleSearch(id), cx);
            }
            self.state = SearchStatus::Done;
        }
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

    /// The selected hit ids, in view order.
    fn selected(&self, cx: &App) -> Vec<u32> {
        let t = self.table.read(cx);
        t.delegate.in_view_order(t.selection.ids())
    }

    /// Ends the processes of the selected rows: SIGKILL with `kill`, else SIGTERM. Each process
    /// is one target however many of its rows are selected. Refused targets (tasklens itself,
    /// PID 1, kernel threads) stop the action with a toast, and the R-036 confirmation runs
    /// first when one applies. A process that exited after the search is still sent, so the
    /// engine answers with the identity toast.
    pub fn end(&mut self, kill: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selected(cx);
        let captured = {
            let d = &self.table.read(cx).delegate;
            let targets = d.distinct_hits(&ids).into_iter().map(|h| {
                self.procs
                    .get(h.key)
                    .map_or_else(|| hit_target(h), record_target)
            });
            CapturedTargets::new(targets, None)
        };
        if captured.items.is_empty() {
            return;
        }
        if captured.truncated {
            // More than MAX_TARGETS processes: send nothing and name the limit, as the engine
            // would for an oversized request.
            let (kind, text) = action_words(cx).reject(RejectReason::TooManyTargets);
            toast(window, cx, kind, text);
            return;
        }
        let words = toast_words(cx);
        if let Some(text) = captured
            .items
            .iter()
            .find_map(|t| words.refusal_text(refusal_reason(t)?, t))
        {
            toast(window, cx, ToastKind::Warning, text);
            return;
        }
        let ask = confirm_kind(&captured.items, kill, cx);
        let kind = ActionKind::Signal(if kill { Signal::Kill } else { Signal::Term });
        let Some(texts) = words.texts_for(&kind, &captured) else {
            return;
        };
        let texts = texts.into_toast_texts(action_words(cx));
        let Some(queue) = ToastQueue::of_window(window, cx).map(|q| q.downgrade()) else {
            warn!(target: "tasklens::ui", "no toast stack; the end action was not sent");
            return;
        };
        let request = ActionRequest {
            // The tracker gives id 0 its next ActionId.
            id: ActionId(0),
            kind,
            targets: captured
                .items
                .iter()
                .filter_map(|t| match t.node {
                    NodeId::Process(key) => Some(ActionTarget::Process(key)),
                    _ => None,
                })
                .collect(),
            privileged: false,
        };
        let Some(ask) = ask else {
            submit(request, queue, texts, cx);
            return;
        };
        let parent = window.window_handle();
        // The dialog opens modal to this window, which is being updated now.
        cx.defer(move |cx| {
            confirm(ask, parent, cx, move |result, cx| {
                if result == ConfirmResult::Confirmed {
                    submit(request, queue, texts, cx);
                }
            });
        });
    }

    fn on_end_task(
        &mut self,
        _: &HandleSearchEndTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end(false, window, cx);
    }

    fn on_end_process(
        &mut self,
        _: &HandleSearchEndProcess,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end(true, window, cx);
    }

    /// The row an item for one row acts on: the focus row when it is selected, else the first
    /// selected row in view order. A Ctrl-click that deselects a row leaves the focus on it, so
    /// the focus alone can name a row that is not selected.
    fn acting_row(&self, cx: &App) -> Option<u32> {
        let t = self.table.read(cx);
        t.selection
            .focus()
            .filter(|id| t.selection.contains(id))
            .or_else(|| t.delegate.in_view_order(t.selection.ids()).first().copied())
    }

    /// Go to process (PROC-071): the acting row's process on the Processes tab.
    fn on_go_to(&mut self, _: &HandleSearchGoToProcess, _: &mut Window, cx: &mut Context<Self>) {
        let key = self
            .acting_row(cx)
            .and_then(|id| self.table.read(cx).delegate.hit(id).map(|h| h.key));
        if let Some(key) = key {
            go_to_process(key, cx);
        }
    }

    /// Ctrl+C and "Copy path": the selected paths, one per line.
    fn on_copy_paths(
        &mut self,
        _: &HandleSearchCopyPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selected(cx);
        if ids.is_empty() {
            return;
        }
        let text = self.table.read(cx).delegate.paths_text(&ids);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Ctrl+Shift+C: the selected rows as tab-separated lines under the header line.
    fn on_copy_rows(
        &mut self,
        _: &HandleSearchCopyRows,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.selected(cx);
        if ids.is_empty() {
            return;
        }
        let text = self.table.read(cx).delegate.rows_tsv(&ids, true);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Open file location (PROC-073) for the path of the one selected row, the same rule that
    /// enables the menu item.
    fn on_open_location(
        &mut self,
        _: &HandleSearchOpenLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = {
            let t = self.table.read(cx);
            let mut ids = t.selection.ids();
            match (ids.next(), ids.next()) {
                (Some(id), None) => t
                    .delegate
                    .hit(id)
                    .filter(|h| h.path.starts_with('/'))
                    .map(|h| h.path.clone()),
                _ => None,
            }
        };
        let Some(path) = path else {
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx).map(|q| q.downgrade()) else {
            return;
        };
        let request = ActionRequest {
            id: ActionId(0),
            kind: ActionKind::ShowInFileManager {
                path,
                properties: false,
            },
            targets: SmallVec::new(),
            privileged: false,
        };
        let texts = ToastTexts::new(SharedString::default(), self.words.open_failed.clone());
        if submit(request, queue, texts, cx).is_some() {
            toast(window, cx, ToastKind::Info, self.words.opening.clone());
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        // Path is the flex column: it takes the width the others leave.
        let body = window.viewport_size().width.as_f32() - 2. * m.sp[5].as_f32();
        if self.flex_body != body {
            self.flex_body = body;
            self.table.update(cx, |t, cx| {
                if t.delegate_mut().fit_flex_column(body) {
                    t.on_new_version(cx);
                }
            });
        }

        // The result table (E23-031).
        let mut empty = div()
            .debug_selector(|| "HandleSearchEmpty".into())
            .text_color(theme.text_secondary);
        *empty.text_style() = t_base(&m);
        let table = div()
            .debug_selector(|| "HandleSearchTable".into())
            .flex_1()
            .min_h_0()
            .map(|d| {
                if self.state == SearchStatus::Idle {
                    d.child(empty.child(words.empty.clone()))
                } else {
                    d.flex().flex_col().child(self.table.clone())
                }
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
            .on_action(cx.listener(Self::on_end_task))
            .on_action(cx.listener(Self::on_end_process))
            .on_action(cx.listener(Self::on_go_to))
            .on_action(cx.listener(Self::on_copy_paths))
            .on_action(cx.listener(Self::on_copy_rows))
            .on_action(cx.listener(Self::on_open_location))
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
