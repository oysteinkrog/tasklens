//! The File descriptors tab of Process Properties: the fd table, the "Hide unnamed handles"
//! switch, the details pane with the decoded fdinfo flags, copy, and the Copy and Open file
//! location menu items (UI-PWN-012, UI-PWN-018, PROC-080, PROC-073).
//!
//! `PropsWindow` feeds the tab its payload and the exited state. Rows are sorted again only when
//! the payload or the sort changes, and filtered again when the payload, the filter text or the
//! hide switch changes. The switch is `properties.hide_unnamed_handles`; every open tab observes
//! the settings global, so a change in one window re-filters all of them.
//!
//! Paths and socket text are untrusted: the collector sanitizes them and the cells draw them as
//! plain text (E15). "Go to connection" is E23-019.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::ResizeHandleRenderer;
use gpui_kit::component::{
    ResizableState, h_flex, resizable_panel, resize_handle_appearance, v_flex, v_resizable,
};
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter,
    FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window, div, px,
};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tl_model::actions::{ActionId, ActionKind, ActionRequest};
use tl_model::details::{FdDetail, FdKind, FdRow, TabPayload};
use tl_model::ids::ProcessKey;
use tl_model::validity::Validity;
use tracing::warn;

use super::filter::{ClearPropsFilter, FilterChanged, NO_MATCH, PropsFilter, lowercase};
use crate::action_tracker::ToastTexts;
use crate::components::checkbox::TlCheckbox;
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue,
};
use crate::components::menu::TlMenu;
use crate::components::toast::{ToastKind, ToastQueue};
use crate::components::tooltip::TlTooltip;
use crate::host::HostContext;
use crate::i18n::{Strings, tr};
use crate::pages::connections::actions::GoToConnection;
use crate::settings_global::{SettingKey, SettingValue, TlSettings};
use crate::table::actions::CopyRow;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate, TreeTableEvent,
};
use crate::text::style::t_base;
use crate::theme::{current, metrics};
use crate::window::main::ShellState;

gpui_kit::actions!(
    tasklens_fds,
    [FdsCopyCell, FdsOpenFileLocation, FdsGoToConnection]
);

/// Details pane heights at 100% scale: the default and the minimum. The maximum is half the
/// tab body.
pub const PANE_DEFAULT_H: f32 = 132.;
pub const PANE_MIN_H: f32 = 80.;
const PANE_ROW_H: f32 = 22.;
const PANE_LABEL_W: f32 = 130.;
/// The kit handle's padding on each side of its 1 px line.
const SPLIT_GRAB: f32 = 4.;

// Open flags (asm-generic/fcntl.h) as fdinfo reports them, in octal.
const O_ACCMODE: u32 = 0o3;
const O_APPEND: u32 = 0o2000;
const O_NONBLOCK: u32 = 0o4000;
const O_DSYNC: u32 = 0o10000;
const O_DIRECT: u32 = 0o40000;
const O_CLOEXEC: u32 = 0o2000000;
const O_SYNC: u32 = 0o4000000 | O_DSYNC;
const O_PATH: u32 = 0o10000000;
const O_TMPFILE: u32 = 0o20000000;

/// The columns of the File descriptors table, in their fixed order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FdColumn {
    Fd,
    Type,
    Name,
    Access,
    Position,
}

impl FdColumn {
    pub const ALL: [FdColumn; 5] = [
        Self::Fd,
        Self::Type,
        Self::Name,
        Self::Access,
        Self::Position,
    ];

    /// Header key, width at 100% and alignment.
    fn meta(self) -> (&'static str, f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Fd => ("pwn-fds-col-fd", 64., Right),
            Self::Type => ("pwn-fds-col-type", 96., Left),
            Self::Name => ("pwn-fds-col-name", 320., Left),
            Self::Access => ("pwn-fds-col-access", 120., Left),
            Self::Position => ("pwn-fds-col-position", 110., Right),
        }
    }
}

impl TableColumn for FdColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Fd => "fd",
            Self::Type => "type",
            Self::Name => "name",
            Self::Access => "access",
            Self::Position => "pos",
        }
    }

    /// The layout is never saved, so no stored key maps to it.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }
}

/// The sort of the table, kept for the window's life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FdSort {
    pub column: FdColumn,
    pub ascending: bool,
}

impl Default for FdSort {
    /// FD ascending.
    fn default() -> Self {
        Self {
            column: FdColumn::Fd,
            ascending: true,
        }
    }
}

impl TableSort<FdColumn> for FdSort {
    fn column(&self) -> FdColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

/// The tab's words, translated once per strings version.
pub struct FdWords {
    strings_version: Option<u64>,
    /// Header labels in [`FdColumn::ALL`] order.
    pub headers: [SharedString; 5],
    pub loading: SharedString,
    pub empty: SharedString,
    pub no_match: SharedString,
    pub hide_unnamed: SharedString,
    pub pane_none: SharedString,
    pub pane_unavailable: SharedString,
    pub splitter_tip: SharedString,
    /// Raw flags, Access mode, Status flags, Descriptor flags.
    pub left: [SharedString; 4],
    /// Mount ID, Inode, Count, DRM client, DRM driver.
    pub right: [SharedString; 5],
    pub copy: SharedString,
    pub open_location: SharedString,
    pub go_to_connection: SharedString,
    pub copied: SharedString,
    pub opening: SharedString,
    pub open_failed: SharedString,
}

impl FdWords {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            strings_version: strings_version(cx),
            headers: FdColumn::ALL.map(|c| t(c.meta().0)),
            loading: t("pwn-fds-loading"),
            empty: t("pwn-fds-empty"),
            no_match: t(NO_MATCH),
            hide_unnamed: t("pwn-fds-hide-unnamed"),
            pane_none: t("pwn-fds-pane-none"),
            pane_unavailable: t("pwn-fds-pane-unavailable"),
            splitter_tip: t("pwn-fds-splitter-tooltip"),
            left: [
                "pwn-fds-raw-flags",
                "pwn-fds-access-mode",
                "pwn-fds-status-flags",
                "pwn-fds-descriptor-flags",
            ]
            .map(t),
            right: [
                "pwn-fds-mount-id",
                "pwn-fds-inode",
                "pwn-fds-eventfd-count",
                "pwn-fds-drm-client",
                "pwn-fds-drm-driver",
            ]
            .map(t),
            copy: t("pwn-fds-menu-copy"),
            open_location: t("pwn-fds-menu-open-location"),
            go_to_connection: t("pwn-fds-menu-go-to-connection"),
            copied: t("pwn-fds-toast-copied"),
            opening: t("pwn-fds-toast-opening"),
            open_failed: t("pwn-fds-toast-open-failed"),
        }
    }
}

fn strings_version(cx: &App) -> Option<u64> {
    cx.try_global::<Strings>().map(|s| s.strings_version)
}

/// `properties.hide_unnamed_handles`, `true` without the settings global.
fn hide_setting(cx: &App) -> bool {
    if cx.has_global::<TlSettings>() {
        TlSettings::settings(cx).properties.hide_unnamed_handles
    } else {
        true
    }
}

// ---- Flags (pure) ----

/// The decoded fdinfo flags: (access mode, status flags, descriptor flags). Status flags are
/// joined by ", " from Append, Non-blocking, Sync, Direct, Path and Temp file; other bits, such
/// as large-file, are not shown. No status flag reads "None".
pub fn decode_flags(raw: u32, close_on_exec: bool) -> (SharedString, SharedString, SharedString) {
    let access = if raw & O_PATH != 0 {
        "Path only"
    } else {
        match raw & O_ACCMODE {
            0 => "Read only",
            1 => "Write only",
            _ => "Read/write",
        }
    };
    let mut status: SmallVec<[&str; 6]> = SmallVec::new();
    if raw & O_APPEND != 0 {
        status.push("Append");
    }
    if raw & O_NONBLOCK != 0 {
        status.push("Non-blocking");
    }
    if raw & O_SYNC == O_SYNC {
        status.push("Sync");
    }
    if raw & O_DIRECT != 0 {
        status.push("Direct");
    }
    if raw & O_PATH != 0 {
        status.push("Path");
    }
    if raw & O_TMPFILE != 0 {
        status.push("Temp file");
    }
    let status = if status.is_empty() {
        "None".to_owned()
    } else {
        status.join(", ")
    };
    let descriptor = if close_on_exec {
        "Close-on-exec"
    } else {
        "None"
    };
    (access.into(), status.into(), descriptor.into())
}

/// Whether fdinfo `flags` has close-on-exec.
pub fn close_on_exec(raw: u32) -> bool {
    raw & O_CLOEXEC != 0
}

/// The Access cell: `RW +A +NB CLOEXEC`. `-` without fdinfo flags.
pub fn fmt_access(out: &mut String, flags: Option<u32>) {
    let Some(raw) = flags else {
        out.push('-');
        return;
    };
    out.push_str(if raw & O_PATH != 0 {
        "PATH"
    } else {
        match raw & O_ACCMODE {
            0 => "R",
            1 => "W",
            _ => "RW",
        }
    });
    for (bit, word) in [(O_APPEND, " +A"), (O_NONBLOCK, " +NB"), (O_DIRECT, " +D")] {
        if raw & bit != 0 {
            out.push_str(word);
        }
    }
    if raw & O_SYNC == O_SYNC {
        out.push_str(" +S");
    }
    if close_on_exec(raw) {
        out.push_str(" CLOEXEC");
    }
}

/// Raw flags in octal: `0o2104002`.
pub fn fmt_raw(out: &mut String, raw: u32) {
    let _ = write!(out, "0o{raw:o}");
}

/// Orders rows `a` and `b` by `col`, then by FD.
fn compare(a: &FdRow, b: &FdRow, col: FdColumn) -> Ordering {
    let by = match col {
        FdColumn::Fd => Ordering::Equal,
        FdColumn::Type => a.kind.label().cmp(b.kind.label()),
        FdColumn::Name => a.name.cmp(&b.name),
        FdColumn::Access => a.flags.cmp(&b.flags),
        FdColumn::Position => a.pos.cmp(&b.pos),
    };
    by.then(a.fd.cmp(&b.fd))
}

/// The socket inode "Go to connection" jumps to: `TCP`, `UDP` and `unix` rows only.
fn socket_inode_of(r: &FdRow) -> Option<u64> {
    matches!(r.kind, FdKind::Tcp | FdKind::Udp | FdKind::Unix)
        .then(|| r.socket_inode.or(r.inode))
        .flatten()
}

/// Whether the row has a file location to open.
fn has_location(r: &FdRow) -> bool {
    matches!(r.kind, FdKind::File | FdKind::Directory) && r.name.starts_with('/')
}

// ---- The table ----

/// The File descriptors table's rows, columns and sort.
pub struct FdsDelegate {
    /// The host the tab shows: its desktop commands say "on <host>" for a remote one.
    host: HostContext,
    /// The payload rows, in payload order.
    rows: Arc<[FdRow]>,
    /// Row indexes in sort order.
    order: Vec<u32>,
    /// Row indexes the text filter keeps, `None` for all.
    filtered: Option<Arc<[u32]>>,
    /// Hide unnamed handles.
    hide_unnamed: bool,
    /// Row indexes shown, in sort order.
    view: Vec<u32>,
    /// FD to position in `view`.
    index: FxHashMap<i32, u32>,
    loading: bool,
    exited: bool,
    /// The row and column the last menu opened on, for its Copy item.
    menu_cell: Option<(i32, FdColumn)>,
    version: u64,
    sort: FdSort,
    specs: Vec<ColumnSpec<FdColumn>>,
    layout: ColumnLayout<FdColumn>,
    words: Rc<FdWords>,
}

impl FdsDelegate {
    fn new(words: Rc<FdWords>, hide_unnamed: bool, host: HostContext) -> Self {
        let specs = specs(&words);
        let layout =
            ColumnLayout::defaults(TableId::Processes, &specs, &FdColumn::ALL, &FdColumn::ALL);
        Self {
            host,
            rows: Arc::from([]),
            order: Vec::new(),
            filtered: None,
            hide_unnamed,
            view: Vec::new(),
            index: FxHashMap::default(),
            loading: true,
            exited: false,
            menu_cell: None,
            version: 0,
            sort: FdSort::default(),
            specs,
            layout,
            words,
        }
    }

    /// The FDs shown, in view order.
    pub fn view_fds(&self) -> impl Iterator<Item = i32> + '_ {
        self.view.iter().map(|&i| self.rows[i as usize].fd)
    }

    /// The row of `fd`, if it is shown.
    pub fn row_of(&self, fd: i32) -> Option<&FdRow> {
        self.row_index(fd).map(|ix| self.row(ix))
    }

    /// The text the `col` cell of `fd` shows, `None` when the row is not shown.
    pub fn cell_text(&self, fd: i32, col: FdColumn) -> Option<String> {
        let r = self.row_of(fd)?;
        let mut out = String::new();
        write_text(r, col, &mut out);
        Some(out)
    }

    /// Rows after the hide switch, before the text filter: the tab count.
    pub fn count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| !(self.hide_unnamed && r.unnamed))
            .count()
    }

    fn row(&self, ix: usize) -> &FdRow {
        &self.rows[self.view[ix] as usize]
    }

    fn set_rows(&mut self, rows: Arc<[FdRow]>) {
        self.rows = rows;
        self.loading = false;
        self.filtered = None;
        self.resort();
    }

    fn resort(&mut self) {
        let rows = &self.rows;
        let FdSort { column, ascending } = self.sort;
        self.order.clear();
        self.order.extend(0..rows.len() as u32);
        self.order.sort_unstable_by(|&a, &b| {
            let o = compare(&rows[a as usize], &rows[b as usize], column);
            if ascending { o } else { o.reverse() }
        });
        self.rebuild_view();
    }

    fn rebuild_view(&mut self) {
        let mut keep = None;
        if let Some(f) = &self.filtered {
            let mut shown = vec![false; self.rows.len()];
            for &i in f.iter() {
                if let Some(s) = shown.get_mut(i as usize) {
                    *s = true;
                }
            }
            keep = Some(shown);
        }
        let rows = &self.rows;
        let hide = self.hide_unnamed;
        self.view.clear();
        self.view.extend(self.order.iter().copied().filter(|&i| {
            let r = &rows[i as usize];
            !(hide && r.unnamed) && keep.as_ref().is_none_or(|k| k[i as usize])
        }));
        self.index.clear();
        for (pos, &i) in self.view.iter().enumerate() {
            self.index.insert(self.rows[i as usize].fd, pos as u32);
        }
        self.version += 1;
    }

    /// The selected rows in view order as `FD\tType\tName\tAccess\tPosition` lines, with the
    /// header line first when `header`.
    pub fn rows_tsv(&self, fds: &[i32], header: bool) -> String {
        let mut ixs: Vec<usize> = fds.iter().filter_map(|fd| self.row_index(*fd)).collect();
        ixs.sort_unstable();
        let mut out = String::new();
        if header {
            out.push_str(&self.words.headers.join("\t"));
        }
        for ix in ixs {
            if !out.is_empty() {
                out.push('\n');
            }
            for (i, &col) in FdColumn::ALL.iter().enumerate() {
                if i > 0 {
                    out.push('\t');
                }
                write_text(self.row(ix), col, &mut out);
            }
        }
        out
    }
}

/// The display text of `col` for row `r`.
fn write_text(r: &FdRow, col: FdColumn, out: &mut String) {
    match col {
        FdColumn::Fd => {
            let _ = write!(out, "{}", r.fd);
        }
        FdColumn::Type => out.push_str(r.kind.label()),
        FdColumn::Name => out.push_str(&r.name),
        FdColumn::Access => fmt_access(out, r.flags),
        FdColumn::Position => match r.pos {
            Some(pos) => {
                let _ = write!(out, "{pos}");
            }
            None => out.push('-'),
        },
    }
}

fn specs(words: &FdWords) -> Vec<ColumnSpec<FdColumn>> {
    FdColumn::ALL
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
                kind: match (id, align) {
                    (FdColumn::Name, _) => ColumnKind::Path,
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

impl TreeTableDelegate for FdsDelegate {
    type RowId = i32;
    type Column = FdColumn;
    type Sort = FdSort;

    fn row_count(&self) -> usize {
        self.view.len()
    }
    fn row_id(&self, ix: usize) -> i32 {
        self.row(ix).fd
    }
    fn row_index(&self, id: i32) -> Option<usize> {
        self.index.get(&id).map(|&p| p as usize)
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
    fn columns(&self) -> &ColumnLayout<FdColumn> {
        &self.layout
    }
    fn column_spec(&self, id: FdColumn) -> &ColumnSpec<FdColumn> {
        let i = FdColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: FdColumn, out: &mut CellOut) {
        let r = self.row(ix);
        out.bits = match col {
            FdColumn::Fd => r.fd as u32 as u64,
            FdColumn::Type => r.kind as u64,
            FdColumn::Name => (r.name.as_ptr() as u64) ^ ((r.name.len() as u64) << 48),
            FdColumn::Access => r.flags.map_or(u64::MAX, u64::from),
            FdColumn::Position => r.pos.unwrap_or(u64::MAX),
        };
        out.validity = Validity::Valid;
        if out.want_text {
            write_text(r, col, &mut out.text);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::from(self.row(ix).name.clone()),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn flat_names(&self) -> bool {
        true
    }

    /// The Name column marks U+FFFD and anchors the keyboard menu.
    fn name_column(&self) -> Option<FdColumn> {
        Some(FdColumn::Name)
    }

    fn header_total(&self, _: FdColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> FdSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        true
    }

    /// Copy, Open file location (enabled for `file` and `directory` rows) and Go to connection
    /// (enabled for `TCP`, `UDP` and `unix` rows). Everything is disabled once the process
    /// exited.
    fn context_menu(
        &mut self,
        targets: &[i32],
        col: Option<FdColumn>,
        menu: TlMenu,
        _: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let w = self.words.clone();
        let open_location = self.host.desktop_label(w.open_location.clone(), cx);
        self.menu_cell = targets
            .first()
            .map(|&fd| (fd, col.unwrap_or(FdColumn::Name)));
        if self.exited {
            return menu
                .item_disabled(w.copy.clone(), None)
                .item_disabled(open_location.clone(), None)
                .item_disabled(w.go_to_connection.clone(), None);
        }
        let menu = menu.item(w.copy.clone(), Box::new(FdsCopyCell));
        let one_file = match targets {
            [fd] => self.row_of(*fd).is_some_and(has_location),
            _ => false,
        };
        let menu = if one_file {
            menu.item(open_location, Box::new(FdsOpenFileLocation))
        } else {
            menu.item_disabled(open_location.clone(), None)
        };
        let one_socket = match targets {
            [fd] => self
                .row_of(*fd)
                .is_some_and(|r| socket_inode_of(r).is_some()),
            _ => false,
        };
        if one_socket {
            menu.item(w.go_to_connection.clone(), Box::new(FdsGoToConnection))
        } else {
            menu.item_disabled(w.go_to_connection.clone(), None)
        }
    }

    fn on_toggle_expand(&mut self, _: i32, _: &mut Context<TlTreeTable<Self>>) {}

    /// A new column sorts descending first; the same column flips.
    fn on_sort(&mut self, col: FdColumn, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = if self.sort.column == col {
            FdSort {
                column: col,
                ascending: !self.sort.ascending,
            }
        } else {
            FdSort {
                column: col,
                ascending: false,
            }
        };
        self.resort();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<FdColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[i32], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: i32, _: &mut Context<TlTreeTable<Self>>) {}

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        let w = &self.words;
        Some(if self.loading {
            w.loading.clone()
        } else if self.count() == 0 {
            w.empty.clone()
        } else {
            w.no_match.clone()
        })
    }

    /// The no-match text comes with "Clear filter" (UI-PWN-005).
    fn empty_clear_filter(&self) -> bool {
        !self.loading && self.count() != 0
    }

    fn empty_clear_action(&self) -> Box<dyn gpui_kit::Action> {
        Box::new(ClearPropsFilter)
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- The details pane ----

/// The details pane under the table: the selected descriptor's decoded fdinfo.
pub struct FdDetailsPane {
    row: Option<FdRow>,
    fdinfo_available: bool,
    focus: FocusHandle,
    words: Rc<FdWords>,
}

impl FdDetailsPane {
    /// The pane as `Label: value` lines, empty without a row.
    pub fn lines(&self) -> Vec<(SharedString, SharedString)> {
        let Some(r) = &self.row else {
            return Vec::new();
        };
        let w = &self.words;
        let mut out = Vec::new();
        if let (true, Some(raw)) = (self.fdinfo_available, r.flags) {
            let mut text = String::new();
            fmt_raw(&mut text, raw);
            let (access, status, descriptor) = decode_flags(raw, close_on_exec(raw));
            out.push((w.left[0].clone(), text.into()));
            out.push((w.left[1].clone(), access));
            out.push((w.left[2].clone(), status));
            out.push((w.left[3].clone(), descriptor));
        }
        let num = |v: Option<u64>| -> SharedString {
            v.map_or_else(|| "-".into(), |v| v.to_string().into())
        };
        out.push((w.right[0].clone(), num(r.mnt_id.map(u64::from))));
        out.push((w.right[1].clone(), num(r.inode)));
        if r.kind == FdKind::Eventfd {
            out.push((w.right[2].clone(), num(r.eventfd_count)));
        }
        if r.kind == FdKind::Drm {
            out.push((w.right[3].clone(), num(r.drm_client_id)));
            let driver = r.drm_driver.as_deref().unwrap_or("-");
            out.push((w.right[4].clone(), SharedString::from(driver.to_owned())));
        }
        out
    }

    /// The pane's Tab stop, after the table, for Ctrl+C.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The shown text when there is no table: no selection or no fdinfo.
    pub fn message(&self) -> Option<SharedString> {
        match &self.row {
            None => Some(self.words.pane_none.clone()),
            Some(r) if !self.fdinfo_available || r.flags.is_none() => {
                Some(self.words.pane_unavailable.clone())
            }
            Some(_) => None,
        }
    }

    fn set(&mut self, row: Option<FdRow>, fdinfo_available: bool, cx: &mut Context<Self>) {
        let same = match (&self.row, &row) {
            (Some(a), Some(b)) => {
                a.fd == b.fd
                    && a.flags == b.flags
                    && a.mnt_id == b.mnt_id
                    && a.inode == b.inode
                    && a.eventfd_count == b.eventfd_count
                    && a.drm_client_id == b.drm_client_id
                    && a.drm_driver == b.drm_driver
            }
            (None, None) => true,
            _ => false,
        };
        if same && fdinfo_available == self.fdinfo_available {
            return;
        }
        self.row = row;
        self.fdinfo_available = fdinfo_available;
        cx.notify();
    }

    /// Ctrl+C in the pane: its lines.
    fn on_copy(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let lines = self.lines();
        if lines.is_empty() {
            return;
        }
        let text: Vec<String> = lines
            .iter()
            .map(|(label, value)| {
                let label = label.trim_end_matches(':');
                format!("{label}: {value}")
            })
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n")));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }
}

impl Render for FdDetailsPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = *metrics(cx);
        let s = m.ui_scale;
        let theme = current(cx).clone();
        let root = div()
            .id("FdDetailsPane")
            .debug_selector(|| "FdDetailsPane".into())
            .key_context(crate::table::actions::TABLE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_copy))
            .size_full()
            .p(m.sp[4])
            .bg(theme.list_bg);
        if let Some(text) = self.message() {
            let mut msg = div()
                .debug_selector(|| "FdDetailsPane-message".into())
                .text_color(theme.text_secondary);
            *msg.text_style() = t_base(&m);
            return root
                .flex()
                .items_center()
                .justify_center()
                .child(msg.child(text))
                .into_any_element();
        }
        let lines = self.lines();
        let (left, right) = lines.split_at(4.min(lines.len()));
        let grid = |id: &'static str, rows: &[(SharedString, SharedString)]| {
            let rows = rows
                .iter()
                .map(|(label, value)| TlKvRow {
                    label: label.clone(),
                    value: TlValue::new(value.clone()),
                })
                .collect();
            let sections = [TlKvSection {
                title: None,
                blocks: vec![TlKvBlock {
                    indent: None,
                    heading: None,
                    rows,
                }],
            }];
            div().flex_1().min_w_0().child(
                TlKeyValueGrid::new(id, KvLayout::Inline)
                    .row_height(px(PANE_ROW_H * s))
                    .label_width(px(PANE_LABEL_W * s))
                    .sections(&sections),
            )
        };
        root.child(
            h_flex()
                .w_full()
                .items_start()
                .gap(m.sp[6])
                .child(grid("fds-pane-left", left))
                .child(grid("fds-pane-right", right)),
        )
        .into_any_element()
    }
}

// ---- The tab ----

/// The tab count changed: the visible rows after the hide switch.
pub struct FdsCountChanged(pub u32);

/// The File descriptors tab view. `PropsWindow` builds it with the process key and the tab's
/// filter.
pub struct FdsTab {
    /// The host this tab shows. Set when it opens; never changes.
    host: HostContext,
    key: ProcessKey,
    table: Entity<TlTreeTable<FdsDelegate>>,
    pane: Entity<FdDetailsPane>,
    split: Entity<ResizableState>,
    filter: Entity<PropsFilter>,
    /// Lowercase `FD type name` per payload row, for the filter.
    haystacks: Arc<[Box<str>]>,
    last_payload: Option<Arc<TabPayload>>,
    fdinfo_available: bool,
    exited: bool,
    words: Rc<FdWords>,
    _subs: SmallVec<[Subscription; 5]>,
}

impl EventEmitter<FdsCountChanged> for FdsTab {}

fn toast(window: &mut Window, cx: &mut App, kind: ToastKind, text: SharedString) {
    if let Some(queue) = ToastQueue::of_window(window, cx) {
        queue.update(cx, |q, cx| q.push(kind, text, None, None, cx));
    }
}

impl FdsTab {
    pub fn new(
        host: &HostContext,
        key: ProcessKey,
        filter: Entity<PropsFilter>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(FdWords::new(cx));
        let delegate = FdsDelegate::new(words.clone(), hide_setting(cx), host.clone());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let pane = cx.new(|cx| FdDetailsPane {
            row: None,
            fdinfo_available: true,
            focus: cx.focus_handle().tab_stop(true),
            words: words.clone(),
        });
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
        // Selection moves fill the pane.
        subs.push(cx.observe(&table, |this, _, cx| this.sync_pane(cx)));
        if cx.has_global::<TlSettings>() {
            subs.push(cx.observe_global::<TlSettings>(|this: &mut Self, cx| {
                let hide = hide_setting(cx);
                this.set_hide_unnamed(hide, cx);
            }));
        }
        subs.push(cx.observe_global::<Strings>(|this: &mut Self, cx| {
            if this.words.strings_version != strings_version(cx) {
                this.words = Rc::new(FdWords::new(cx));
                let words = this.words.clone();
                this.table.update(cx, |t, cx| {
                    let d = t.delegate_mut();
                    d.specs = specs(&words);
                    d.words = words.clone();
                    d.version += 1;
                    t.on_new_version(cx);
                });
                this.pane.update(cx, |p, cx| {
                    p.words = words;
                    cx.notify();
                });
            }
        }));
        Self {
            host: host.clone(),
            key,
            table,
            pane,
            split: cx.new(|_| ResizableState::default()),
            filter,
            haystacks: Arc::from([]),
            last_payload: None,
            fdinfo_available: true,
            exited: false,
            words,
            _subs: subs,
        }
    }

    pub fn key(&self) -> ProcessKey {
        self.key
    }

    pub fn table(&self) -> &Entity<TlTreeTable<FdsDelegate>> {
        &self.table
    }

    pub fn pane(&self) -> &Entity<FdDetailsPane> {
        &self.pane
    }

    pub fn words(&self) -> &Rc<FdWords> {
        &self.words
    }

    /// The tab count: rows after the hide switch.
    pub fn count(&self, cx: &App) -> u32 {
        self.table.read(cx).delegate.count() as u32
    }

    /// Takes the tab's payload. The same `Arc` again does nothing; while exited the rows stay
    /// frozen.
    pub fn set_payload(&mut self, payload: Option<&Arc<TabPayload>>, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let same = match (&self.last_payload, payload) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.last_payload = payload.cloned();
        let Some(TabPayload::Fds(detail)) = payload.map(|p| &**p) else {
            return;
        };
        let FdDetail {
            rows,
            fdinfo_available,
        } = FdDetail::clone(detail);
        self.fdinfo_available = fdinfo_available;
        let mut hay = String::new();
        self.haystacks = rows
            .iter()
            .map(|r| {
                hay.clear();
                let _ = write!(hay, "{} {} {}", r.fd, r.kind.label(), r.name);
                lowercase(&hay).into_boxed_str()
            })
            .collect();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_rows(rows);
            t.on_new_version(cx);
        });
        if !self.filter.read(cx).needle().is_empty() {
            let generation = self.filter.read(cx).generation();
            self.run_filter(generation, cx);
        }
        self.sync_pane(cx);
        let n = self.count(cx);
        cx.emit(FdsCountChanged(n));
    }

    /// Hide unnamed handles changed (here or in another window).
    pub fn set_hide_unnamed(&mut self, hide: bool, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate.hide_unnamed == hide {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.hide_unnamed = hide;
            d.rebuild_view();
            t.on_new_version(cx);
        });
        let n = self.count(cx);
        cx.emit(FdsCountChanged(n));
        cx.notify();
    }

    /// Freezes the rows and disables the menu once the process exited.
    pub fn set_exited(&mut self, exited: bool, cx: &mut Context<Self>) {
        if self.exited != exited {
            self.exited = exited;
            self.table
                .update(cx, |t, _| t.delegate_mut().exited = exited);
            cx.notify();
        }
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// The checkbox for the filter bar's right end.
    pub fn filter_tool(&self, cx: &App) -> AnyElement {
        TlCheckbox::new("FdsHideUnnamed")
            .checked(Some(hide_setting(cx)))
            .label(self.words.hide_unnamed.clone())
            .on_change(|on, _, cx| {
                if cx.has_global::<TlSettings>() {
                    TlSettings::set(
                        cx,
                        SettingKey::PropertiesHideUnnamedHandles,
                        SettingValue::Bool(on),
                    );
                }
            })
            .into_any_element()
    }

    /// The focus row into the pane, when it changed.
    fn sync_pane(&mut self, cx: &mut Context<Self>) {
        let row = {
            let t = self.table.read(cx);
            t.selection
                .focus()
                .and_then(|fd| t.delegate.row_of(fd).cloned())
        };
        let available = self.fdinfo_available;
        self.pane.update(cx, |p, cx| p.set(row, available, cx));
    }

    fn run_filter(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.filter.read(cx).is_current(generation) {
            return;
        }
        if self.filter.read(cx).needle().is_empty() {
            self.apply_filter(generation, None, cx);
            return;
        }
        let rows = self.haystacks.clone();
        let task = self.filter.update(cx, |f, cx| f.run_filter(rows, cx));
        cx.spawn(async move |this, cx| {
            if let Some((generation, keep)) = task.await {
                let _ = this.update(cx, |this, cx| this.apply_filter(generation, Some(keep), cx));
            }
        })
        .detach();
    }

    fn apply_filter(&mut self, generation: u64, keep: Option<Arc<[u32]>>, cx: &mut Context<Self>) {
        if !self.filter.update(cx, |f, _| f.accept(generation)) {
            return;
        }
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.filtered = keep;
            d.rebuild_view();
            t.on_new_version(cx);
        });
    }

    fn selected(&self, cx: &App) -> Vec<i32> {
        self.table.read(cx).selection.ids().collect()
    }

    /// Ctrl+C: the selected rows as TSV lines without the header.
    fn on_copy_row(&mut self, _: &CopyRow, window: &mut Window, cx: &mut Context<Self>) {
        let fds = self.selected(cx);
        if fds.is_empty() {
            return;
        }
        let text = self.table.read(cx).delegate.rows_tsv(&fds, false);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Menu "Copy": the text of the cell the menu opened on.
    fn on_copy_cell(&mut self, _: &FdsCopyCell, window: &mut Window, cx: &mut Context<Self>) {
        let text = {
            let d = &self.table.read(cx).delegate;
            d.menu_cell.and_then(|(fd, col)| d.cell_text(fd, col))
        };
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        toast(window, cx, ToastKind::Success, self.words.copied.clone());
    }

    /// Open file location (PROC-073) for the focus row, `file` and `directory` rows only.
    fn on_open_location(
        &mut self,
        _: &FdsOpenFileLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let path = {
            let t = self.table.read(cx);
            t.selection
                .focus()
                .and_then(|fd| t.delegate.row_of(fd))
                .filter(|r| has_location(r))
                .map(|r| r.name.clone())
        };
        let Some(path) = path else {
            return;
        };
        let Some(tracker) = self.host.tracker().cloned() else {
            warn!(target: "tasklens::ui", "no action tracker, file location not opened");
            return;
        };
        let Some(queue) = ToastQueue::of_window(window, cx) else {
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
        let sent = tracker.update(cx, |t, cx| {
            t.track_or_toast(request, queue.downgrade(), texts, cx)
        });
        if sent.is_some() {
            toast(window, cx, ToastKind::Info, self.words.opening.clone());
        }
    }

    /// Go to connection (UI-PWN-012): dispatches `GoToConnection` for the focus row's socket on
    /// the main window and brings it to the front.
    fn on_go_to_connection(
        &mut self,
        _: &FdsGoToConnection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exited {
            return;
        }
        let inode = {
            let t = self.table.read(cx);
            t.selection
                .focus()
                .and_then(|fd| t.delegate.row_of(fd))
                .and_then(socket_inode_of)
        };
        let Some(inode) = inode else {
            return;
        };
        let Some(main) = cx.try_global::<ShellState>().and_then(|s| s.main_window) else {
            warn!(target: "tasklens::ui", "no main window for Go to connection");
            return;
        };
        let _ = main.update(cx, |_, window, cx| {
            window.activate_window();
            window.dispatch_action(Box::new(GoToConnection { inode }), cx);
        });
    }
}

impl Render for FdsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = metrics(cx).ui_scale;
        let max = (window.viewport_size().height * 0.5).max(px(PANE_MIN_H * s));
        v_flex()
            .debug_selector(|| "PropsFds".into())
            .size_full()
            .on_action(cx.listener(Self::on_copy_row))
            .on_action(cx.listener(Self::on_copy_cell))
            .on_action(cx.listener(Self::on_open_location))
            .on_action(cx.listener(Self::on_go_to_connection))
            .child(
                v_resizable("fds-split")
                    .with_state(&self.split)
                    .with_handle_appearance(split_handle(self.words.splitter_tip.clone()))
                    .child(resizable_panel().child(self.table.clone()))
                    .child(
                        resizable_panel()
                            .flex_none()
                            .size(px(PANE_DEFAULT_H * s))
                            .size_range(px(PANE_MIN_H * s)..max)
                            .child(
                                AnyView::from(self.pane.clone())
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                    ),
            )
    }
}

/// The kit divider with the "Drag to resize" tooltip over the whole grab band.
fn split_handle(tip: SharedString) -> ResizeHandleRenderer {
    let kit = resize_handle_appearance();
    Rc::new(move |handle, window, cx| {
        let line = kit(handle, window, cx)?;
        let tip = tip.clone();
        Some(
            div()
                .id("fds-split-handle")
                .flex_none()
                .w_full()
                .h(px(2. * SPLIT_GRAB + 1.))
                .my(px(-SPLIT_GRAB))
                .flex()
                .items_center()
                .tooltip(TlTooltip::text(tip).build())
                .child(line)
                .into_any_element(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fds_details_decodes_flags() {
        let raw = 0o2104002;
        assert!(close_on_exec(raw));
        let (access, status, descriptor) = decode_flags(raw, close_on_exec(raw));
        assert_eq!(access.as_ref(), "Read/write");
        assert_eq!(status.as_ref(), "Non-blocking");
        assert_eq!(descriptor.as_ref(), "Close-on-exec");
        assert!(!status.to_lowercase().contains("large"));
        let mut s = String::new();
        fmt_raw(&mut s, raw);
        assert_eq!(s, "0o2104002");

        let (access, status, descriptor) = decode_flags(0o2001, false);
        assert_eq!(access.as_ref(), "Write only");
        assert_eq!(status.as_ref(), "Append");
        assert_eq!(descriptor.as_ref(), "None");
        let (access, status, _) = decode_flags(0o10000000, false);
        assert_eq!(access.as_ref(), "Path only");
        assert_eq!(status.as_ref(), "Path");
        assert_eq!(decode_flags(0, false).1.as_ref(), "None");
    }

    #[test]
    fn fds_access_cell() {
        let mut s = String::new();
        fmt_access(&mut s, Some(0o2106002));
        assert_eq!(s, "RW +A +NB CLOEXEC");
        s.clear();
        fmt_access(&mut s, Some(0));
        assert_eq!(s, "R");
        s.clear();
        fmt_access(&mut s, None);
        assert_eq!(s, "-");
    }
}
