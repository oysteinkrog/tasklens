//! The History table: `HistoryTableDelegate`, a flat `TreeTableDelegate` over the rows of the
//! installed `HistoryView` (E30-019, UI-PRC-031, PROC-088, PROC-046, R-006, ADR-009, ADR-011).
//!
//! The rows are the processes at the scrub second, or averages over a selected range with the
//! peak in each metric cell's tooltip. The delegate filters and sorts them itself: at most a few
//! hundred rows, once per installed view, filter, sort or `processes.hide_system` change. Rows
//! of absent identities (`present = false`) always come after every present row.
//!
//! Rendering cost: every per-row answer reads the view through the display order built at
//! install. Words are translated at construction and on a locale change only. A peak tooltip is
//! formatted once per hovered row and table version. Values use the live table's formatters
//! (UI-PRC-004).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::{AnyView, App, Context, SharedString, Window, px};
use rustc_hash::FxHashMap;
use smallvec::{SmallVec, smallvec};
use tl_model::history::{HistoryPeaks, HistoryProcessRow, HistoryRange, HistoryView};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::process::ProcFlags;
use tl_model::settings::{NetworkUnit, Settings, SortState, UiState};
use tl_model::snapshot::ProcessSnapshot;
use tl_model::table::SortSpec;
use tl_model::units::{self, NetUnit};
use tl_model::validity::Validity;

use super::actions::{DeleteFromHistory, HistoryShowInLiveTable};
use crate::components::icons::TlIcon;
use crate::components::menu::TlMenu;
use crate::components::validity::copy_text;
use crate::hub::GlobalHub;
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::pages::processes::actions::{CopyAllMetrics, CopyDetails};
use crate::pages::processes::cells::process_column;
use crate::pages::processes::delegate::ValidityTips;
use crate::settings_global::TlSettings;
use crate::table::actions::{CopyCell, CopyRowsAsTable};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId, next_sort,
};
use crate::table::tree_table::{
    IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};
use crate::theme::metrics;

/// The columns in default order (UI-PRC-031). Ids are the R-007 `ColumnId` strings.
pub const HISTORY_COLUMNS: [ColumnId; 9] = [
    ColumnId::Name,
    ColumnId::Pid,
    ColumnId::User,
    ColumnId::ImagePath,
    ColumnId::Cpu,
    ColumnId::Memory,
    ColumnId::Disk,
    ColumnId::Network,
    ColumnId::Gpu,
];

/// Default widths of [`HISTORY_COLUMNS`] in logical px at 100% (R-006).
const WIDTHS: [f32; 9] = [248.0, 96.0, 92.0, 200.0, 63.0, 92.0, 82.0, 92.0, 72.0];

const KEY_IMAGE_PATH: &str = "history-col-image-path";
const KEY_VALUES_AT: &str = "history-tip-values-at";
const KEY_AVERAGES: &str = "history-tip-averages";
const KEY_PEAK: &str = "history-cell-peak";
const KEY_NOT_RUNNING: &str = "history-not-running";
const KEY_PREVIOUS_BOOT: &str = "history-previous-boot";

/// The table's words, translated once per strings version. `Default` is en-US.
#[derive(Clone, Debug)]
pub struct HistoryTableWords {
    /// "Values at the selected time".
    pub values_at: SharedString,
    /// "Averages over the selected range (peak in cell tooltips)".
    pub averages: SharedString,
    /// "peak {$value}".
    pub peak: Template,
    /// "Process not running".
    pub not_running: SharedString,
    /// "(previous boot)".
    pub previous_boot: SharedString,
}

impl Default for HistoryTableWords {
    fn default() -> Self {
        Self {
            values_at: tr_en(KEY_VALUES_AT, &[]),
            averages: tr_en(KEY_AVERAGES, &[]),
            peak: Template::en(KEY_PEAK, &["value"]),
            not_running: tr_en(KEY_NOT_RUNNING, &[]),
            previous_boot: tr_en(KEY_PREVIOUS_BOOT, &[]),
        }
    }
}

impl HistoryTableWords {
    /// Every text through `tr`, or en-US before the strings are installed.
    pub fn new(cx: &App) -> Self {
        if !cx.has_global::<Strings>() {
            return Self::default();
        }
        Self {
            values_at: tr(cx, KEY_VALUES_AT),
            averages: tr(cx, KEY_AVERAGES),
            peak: Template::new(cx, KEY_PEAK, &["value"]),
            not_running: tr(cx, KEY_NOT_RUNNING),
            previous_boot: tr(cx, KEY_PREVIOUS_BOOT),
        }
    }
}

/// True when `key` is in the current live process list and has not exited.
pub fn is_live(key: ProcessKey, cx: &App) -> bool {
    cx.try_global::<GlobalHub>().is_some_and(|hub| {
        hub.0
            .read(cx)
            .current()
            .processes
            .get(key)
            .is_some_and(|r| !r.flags.contains(ProcFlags::EXITED))
    })
}

const MIB: f64 = 1024.0 * 1024.0;

/// One metric value before formatting.
#[derive(Clone, Copy)]
enum Metric {
    Percent(f32),
    Bytes(u64),
    DiskRate(f32),
    NetRate(f32),
}

impl Metric {
    /// `col`'s value in `r`. `None` for a column that is not a metric.
    fn of_row(r: &HistoryProcessRow, col: ColumnId) -> Option<Self> {
        Some(match col {
            ColumnId::Cpu => Self::Percent(r.cpu_pct),
            ColumnId::Memory => Self::Bytes(r.mem_anon_bytes),
            ColumnId::Disk => Self::DiskRate(r.disk_bps),
            ColumnId::Network => Self::NetRate(r.net_bps),
            ColumnId::Gpu => Self::Percent(r.gpu_pct),
            _ => return None,
        })
    }

    fn of_peak(p: &HistoryPeaks, col: ColumnId) -> Option<Self> {
        Some(match col {
            ColumnId::Cpu => Self::Percent(p.cpu_pct),
            ColumnId::Memory => Self::Bytes(p.mem_anon_bytes),
            ColumnId::Disk => Self::DiskRate(p.disk_bps),
            ColumnId::Network => Self::NetRate(p.net_bps),
            ColumnId::Gpu => Self::Percent(p.gpu_pct),
            _ => return None,
        })
    }

    fn bits(self) -> u64 {
        match self {
            Self::Percent(v) | Self::DiskRate(v) | Self::NetRate(v) => u64::from(v.to_bits()),
            Self::Bytes(n) => n,
        }
    }

    /// The live table's formats: percent one decimal, IEC bytes, `MiB/s` one decimal, network
    /// in `units.network`.
    fn write(self, out: &mut String, net: (NetUnit, bool)) {
        match self {
            Self::Percent(v) => units::percent(out, v),
            Self::Bytes(n) => units::bytes(out, n),
            Self::DiskRate(bps) => units::rate_mib_fixed(out, f64::from(bps)),
            Self::NetRate(bps) => units::net_rate(out, f64::from(bps), net.0, net.1),
        }
    }
}

/// The peak tooltips of the hovered row, each formatted on its first ask.
#[derive(Default)]
struct PeakTips {
    /// `(table version, row index)` the tooltips were built for.
    built_for: Option<(u64, usize)>,
    tips: SmallVec<[(ColumnId, SharedString); 5]>,
    scratch: String,
}

/// The History table delegate. The page installs each history view with [`Self::install`] and
/// calls `TlTreeTable::on_new_version` when it returns true.
pub struct HistoryTableDelegate {
    view: Arc<HistoryView>,
    /// Indices into `view.rows`, filtered and in display order.
    order: Vec<u32>,
    /// Display index of each shown row.
    index: FxHashMap<ProcessKey, u32>,
    /// Bumps on every rebuild of `order`.
    version: u64,
    sort: SortSpec,
    /// Lowercase global filter text.
    filter: Arc<str>,
    /// `processes.hide_system` (PROC-046).
    hide_system: bool,
    /// `units.network` and `units.network_auto_scale`.
    net: (NetUnit, bool),
    layout: ColumnLayout,
    /// One spec per entry of [`HISTORY_COLUMNS`], in that order.
    specs: [ColumnSpec; 9],
    words: HistoryTableWords,
    tips: ValidityTips,
    /// Icon file of each app id, from the live process records (the engine resolves icons).
    icons: FxHashMap<Arc<str>, Arc<str>>,
    /// `records` pointer the icons were read from.
    icons_for: usize,
    peak_tips: RefCell<PeakTips>,
    /// Scratch for the lowercase field text the filter compares against.
    lower: String,
    /// The rows and cell column the open row menu was built for. The menu actions read it
    /// through [`Self::take_captured`], so a view that changes under the open menu does not
    /// change what it acts on.
    captured: Option<CapturedRows>,
    /// The row Enter or a double-click activated, for the page's `OpenProperties` handler.
    activated: Option<ProcessKey>,
}

/// The rows (in view order) and the clicked column a row menu was built for.
pub type CapturedRows = (SmallVec<[ProcessKey; 8]>, Option<ColumnId>);

impl HistoryTableDelegate {
    /// Translates the words and labels once, reads the sort from `[sort.history]` (an unknown
    /// column falls back to CPU descending) and the layout from `[columns.history]`.
    pub fn new(settings: &Settings, state: &UiState, cx: &App) -> Self {
        let words = HistoryTableWords::new(cx);
        let specs = build_specs(settings, &words, cx);
        let saved = &state.sort.history;
        let sort = match ColumnId::from_str(&saved.column).filter(|c| HISTORY_COLUMNS.contains(c)) {
            Some(column) => SortSpec {
                column,
                ascending: saved.ascending,
            },
            None => {
                tracing::warn!(target: "tasklens::history", column = %saved.column, "unknown sort column");
                SortSpec {
                    column: ColumnId::Cpu,
                    ascending: false,
                }
            }
        };
        let mut layout = ColumnLayout::from_state(TableId::History, &state.columns.history, &specs);
        layout.rebuild(ui_scale(cx));
        Self {
            view: Arc::new(HistoryView::default()),
            order: Vec::new(),
            index: FxHashMap::default(),
            version: 0,
            sort,
            filter: Arc::from(""),
            hide_system: settings.processes.hide_system,
            net: net_unit(settings),
            layout,
            specs,
            words,
            tips: ValidityTips::default(),
            icons: FxHashMap::default(),
            icons_for: 0,
            peak_tips: RefCell::default(),
            lower: String::new(),
            captured: None,
            activated: None,
        }
    }

    /// The default `[columns.history]` layout for "Reset to default".
    pub fn default_layout(&self) -> ColumnLayout {
        let mut layout = ColumnLayout::defaults(
            TableId::History,
            &self.specs,
            &HISTORY_COLUMNS,
            &HISTORY_COLUMNS,
        );
        layout.rebuild(self.layout.ui_scale());
        layout
    }

    pub fn view(&self) -> &Arc<HistoryView> {
        &self.view
    }

    /// The row shown at display index `ix`.
    pub fn row(&self, ix: usize) -> &HistoryProcessRow {
        &self.view.rows[self.order[ix] as usize]
    }

    /// Shows `view`. `processes` is the live process list the app icons come from. True when the
    /// rows on screen changed, so the page calls `TlTreeTable::on_new_version`.
    pub fn install(&mut self, view: &Arc<HistoryView>, processes: &ProcessSnapshot) -> bool {
        if Arc::ptr_eq(&self.view, view) {
            return false;
        }
        self.view = Arc::clone(view);
        self.name_tooltip();
        self.read_icons(processes);
        self.rebuild();
        true
    }

    /// Sets the lowercase global filter text. True when the rows changed.
    pub fn set_filter(&mut self, lower: Arc<str>) -> bool {
        if lower == self.filter {
            return false;
        }
        self.filter = lower;
        self.rebuild();
        true
    }

    /// Follows `processes.hide_system` and the network unit. True when the rows changed. A unit
    /// change reaches the cell texts through the style version.
    pub fn set_settings(&mut self, settings: &Settings) -> bool {
        self.net = net_unit(settings);
        for spec in &mut self.specs {
            spec.template =
                process_column(spec.id).template(settings.units.network == NetworkUnit::Bytes);
        }
        if settings.processes.hide_system == self.hide_system {
            return false;
        }
        self.hide_system = settings.processes.hide_system;
        self.rebuild();
        true
    }

    /// Translates every word and label again after a locale change.
    pub fn refresh_strings(&mut self, cx: &App) {
        self.words = HistoryTableWords::new(cx);
        for spec in &mut self.specs {
            label_spec(spec, cx);
        }
        self.name_tooltip();
        self.peak_tips.borrow_mut().built_for = None;
        self.version += 1;
    }

    /// The Name header tooltip for the view's range.
    /// Takes the rows the open row menu was built for.
    pub fn take_captured(&mut self) -> Option<CapturedRows> {
        self.captured.take()
    }

    /// Takes the row Enter or a double-click activated.
    pub fn take_activated(&mut self) -> Option<ProcessKey> {
        self.activated.take()
    }

    /// The text a copy puts on the clipboard for the cell at (`ix`, `col`): the displayed text,
    /// or the validity word for a cell with no value.
    fn copy_value(&self, ix: usize, col: ColumnId, out: &mut CellOut) -> String {
        out.reset();
        out.want_text = true;
        self.cell(ix, col, out);
        let text = match out.arc.take() {
            Some(arc) => arc.to_string(),
            None => std::mem::take(&mut out.text),
        };
        copy_text(out.validity, &text.into()).to_string()
    }

    /// Copy: the displayed text of column `col` of each of `keys`, one per line, in the order
    /// given. A key that is no longer shown is skipped.
    pub fn copy_cell_text(&self, keys: &[ProcessKey], col: ColumnId) -> String {
        let mut out = CellOut::default();
        keys.iter()
            .filter_map(|k| self.index.get(k))
            .map(|&ix| self.copy_value(ix as usize, col, &mut out))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Copy details (PROC-070 fields history has): `Name`, `PID`, `Parent PID` (history keeps
    /// none, so it is empty), `CPU`, `Memory`, `Disk`, `Network` and `GPU`, one block per row
    /// separated by a blank line. Never a command line (R-169 OI-6).
    pub fn copy_details_text(&self, keys: &[ProcessKey], cx: &App) -> String {
        let mut out = CellOut::default();
        let ppid = text(cx, process_column(ColumnId::Ppid).label);
        let mut blocks = Vec::new();
        for ix in keys.iter().filter_map(|k| self.index.get(k)) {
            let ix = *ix as usize;
            let r = self.row(ix);
            let mut block = String::new();
            for col in [ColumnId::Name, ColumnId::Pid] {
                let value = self.copy_value(ix, col, &mut out);
                let _ = writeln!(block, "{}: {value}", self.column_spec(col).label);
            }
            let _ = writeln!(block, "{ppid}: ");
            let cpu = if r.present {
                format!("{:.2}%", r.cpu_pct)
            } else {
                self.copy_value(ix, ColumnId::Cpu, &mut out)
            };
            let memory = if r.present {
                format!("{} MiB", (r.mem_anon_bytes as f64 / MIB).round() as u64)
            } else {
                self.copy_value(ix, ColumnId::Memory, &mut out)
            };
            let _ = writeln!(block, "{}: {cpu}", self.column_spec(ColumnId::Cpu).label);
            let _ = write!(
                block,
                "{}: {memory}",
                self.column_spec(ColumnId::Memory).label
            );
            for col in [ColumnId::Disk, ColumnId::Network, ColumnId::Gpu] {
                let value = self.copy_value(ix, col, &mut out);
                let _ = write!(block, "\n{}: {value}", self.column_spec(col).label);
            }
            blocks.push(block);
        }
        blocks.join("\n\n")
    }

    /// Copy all metrics: per row one `Label: value` line for every visible column with a Valid
    /// value and non-empty text, rows separated by a blank line. Never a command line.
    pub fn copy_all_metrics_text(&self, keys: &[ProcessKey]) -> String {
        let mut out = CellOut::default();
        let visible = self.layout.visible_columns().clone();
        let mut blocks = Vec::new();
        for ix in keys.iter().filter_map(|k| self.index.get(k)) {
            let ix = *ix as usize;
            let mut lines = Vec::new();
            for &col in visible.iter() {
                out.reset();
                out.want_text = true;
                self.cell(ix, col, &mut out);
                if out.validity != Validity::Valid {
                    continue;
                }
                let value = match out.arc.take() {
                    Some(arc) => arc.to_string(),
                    None => std::mem::take(&mut out.text),
                };
                if !value.is_empty() {
                    lines.push(format!("{}: {value}", self.column_spec(col).label));
                }
            }
            blocks.push(lines.join("\n"));
        }
        blocks.join("\n\n")
    }

    fn name_tooltip(&mut self) {
        let w = &self.words;
        let tip = match self.view.range {
            HistoryRange::Span { .. } => &w.averages,
            HistoryRange::Live | HistoryRange::At(_) => &w.values_at,
        };
        self.specs[0].tooltip = Some(tip.clone());
    }

    /// The app icons by app id, read again when the live records changed.
    fn read_icons(&mut self, processes: &ProcessSnapshot) {
        let ptr = processes.records.as_ptr() as usize;
        if ptr == self.icons_for {
            return;
        }
        self.icons_for = ptr;
        self.icons.clear();
        for r in processes.records.iter() {
            if let (Some(app), Some(icon)) = (&r.meta.app_id, &r.meta.icon) {
                self.icons
                    .entry(Arc::clone(app))
                    .or_insert_with(|| Arc::clone(icon));
            }
        }
    }

    /// Filters and sorts the rows into `order` and bumps the version. O(rows log rows).
    fn rebuild(&mut self) {
        let rows = &self.view.rows;
        let order = &mut self.order;
        order.clear();
        for (i, r) in rows.iter().enumerate() {
            if self.hide_system && r.system {
                continue;
            }
            if !self.filter.is_empty() && !matches_filter(r, &self.filter, &mut self.lower) {
                continue;
            }
            order.push(i as u32);
        }
        let SortSpec { column, ascending } = self.sort;
        order.sort_unstable_by(|&a, &b| {
            let (a, b) = (&rows[a as usize], &rows[b as usize]);
            b.present
                .cmp(&a.present)
                .then_with(|| {
                    let o = compare(a, b, column);
                    if ascending { o } else { o.reverse() }
                })
                .then_with(|| a.key.cmp(&b.key))
        });
        self.index.clear();
        self.index.reserve(order.len());
        for (ix, &i) in order.iter().enumerate() {
            self.index.insert(rows[i as usize].key, ix as u32);
        }
        self.version += 1;
    }

    /// The "peak <value>" tooltip of cell (`ix`, `col`), from the hovered row's cache.
    fn peak_tip(&self, ix: usize, col: ColumnId, peak: Metric) -> SharedString {
        let mut cache = self.peak_tips.borrow_mut();
        let key = (self.version, ix);
        if cache.built_for != Some(key) {
            cache.built_for = Some(key);
            cache.tips.clear();
        }
        if let Some((_, tip)) = cache.tips.iter().find(|(c, _)| *c == col) {
            return tip.clone();
        }
        let PeakTips { tips, scratch, .. } = &mut *cache;
        scratch.clear();
        let net = self.net;
        self.words
            .peak
            .write_each(scratch, |_, out| peak.write(out, net));
        let tip = SharedString::from(scratch.clone());
        tips.push((col, tip.clone()));
        tip
    }
}

/// True when the lowercase `filter` is in the row's name, comm, user or executable path.
fn matches_filter(r: &HistoryProcessRow, filter: &str, lower: &mut String) -> bool {
    let exe = r.exe.as_deref().unwrap_or("");
    [&*r.name, &*r.comm, &*r.user, exe].into_iter().any(|s| {
        lower.clear();
        lower.extend(s.chars().flat_map(char::to_lowercase));
        lower.contains(filter)
    })
}

/// `col`'s order of `a` and `b`, ascending. Text compares without ASCII case.
fn compare(a: &HistoryProcessRow, b: &HistoryProcessRow, col: ColumnId) -> Ordering {
    match col {
        ColumnId::Pid => a.key.pid.cmp(&b.key.pid),
        ColumnId::User => cmp_text(&a.user, &b.user),
        ColumnId::ImagePath => cmp_text(
            a.exe.as_deref().unwrap_or(""),
            b.exe.as_deref().unwrap_or(""),
        ),
        ColumnId::Cpu => a.cpu_pct.total_cmp(&b.cpu_pct),
        ColumnId::Memory => a.mem_anon_bytes.cmp(&b.mem_anon_bytes),
        ColumnId::Disk => a.disk_bps.total_cmp(&b.disk_bps),
        ColumnId::Network => a.net_bps.total_cmp(&b.net_bps),
        ColumnId::Gpu => a.gpu_pct.total_cmp(&b.gpu_pct),
        _ => cmp_text(&a.name, &b.name),
    }
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|c| c.to_ascii_lowercase())
        .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

fn net_unit(settings: &Settings) -> (NetUnit, bool) {
    let unit = match settings.units.network {
        NetworkUnit::Bits => NetUnit::Bits,
        NetworkUnit::Bytes => NetUnit::Bytes,
    };
    (unit, settings.units.network_auto_scale)
}

fn ui_scale(cx: &App) -> f32 {
    if cx.has_global::<crate::theme::UiMetrics>() {
        metrics(cx).ui_scale
    } else {
        1.0
    }
}

fn arc_bits(text: &Arc<str>) -> u64 {
    Arc::as_ptr(text).cast::<u8>() as usize as u64
}

/// `key` through `tr`, or en-US before the strings are installed.
fn text(cx: &App, key: &str) -> SharedString {
    if cx.has_global::<Strings>() {
        tr(cx, key)
    } else {
        tr_en(key, &[])
    }
}

/// The nine specs: the live table's labels, groups and templates, the History widths, and the
/// History label for Image path.
fn build_specs(settings: &Settings, words: &HistoryTableWords, cx: &App) -> [ColumnSpec; 9] {
    let net_bytes = settings.units.network == NetworkUnit::Bytes;
    let mut i = 0;
    HISTORY_COLUMNS.map(|id| {
        let c = process_column(id);
        let align = c.align();
        let mut spec = ColumnSpec {
            id,
            label: SharedString::default(),
            full_name: SharedString::default(),
            group: SharedString::default(),
            align,
            kind: match align {
                ColumnAlign::Left => ColumnKind::Text,
                ColumnAlign::Right => ColumnKind::Numeric,
            },
            sortable: true,
            template: c.template(net_bytes),
            default_width: px(WIDTHS[i]),
            tooltip: None,
            availability: Availability::Available,
            hideable: id != ColumnId::Name,
        };
        i += 1;
        label_spec(&mut spec, cx);
        if id == ColumnId::Name {
            spec.tooltip = Some(words.values_at.clone());
        }
        spec
    })
}

/// Sets the label, picker name, picker group and header tooltip of `spec`. Leaves the Name
/// tooltip, which follows the view.
fn label_spec(spec: &mut ColumnSpec, cx: &App) {
    let c = process_column(spec.id);
    spec.label = if spec.id == ColumnId::ImagePath {
        text(cx, KEY_IMAGE_PATH)
    } else {
        text(cx, c.label)
    };
    spec.full_name = c.alias.map_or_else(|| spec.label.clone(), |k| text(cx, k));
    spec.group = text(cx, c.group.label_key());
    if spec.id != ColumnId::Name {
        spec.tooltip = c.header_tooltip.map(|k| text(cx, k));
    }
}

impl TreeTableDelegate for HistoryTableDelegate {
    type RowId = ProcessKey;
    type Column = ColumnId;
    type Sort = SortSpec;

    fn row_count(&self) -> usize {
        self.order.len()
    }

    fn row_id(&self, ix: usize) -> ProcessKey {
        self.row(ix).key
    }

    fn row_index(&self, id: ProcessKey) -> Option<usize> {
        self.index.get(&id).map(|&ix| ix as usize)
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

    fn columns(&self) -> &ColumnLayout {
        &self.layout
    }

    fn column_spec(&self, id: ColumnId) -> &ColumnSpec {
        let at = HISTORY_COLUMNS.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[at]
    }

    fn cell(&self, ix: usize, col: ColumnId, out: &mut CellOut) {
        let r = self.row(ix);
        out.validity = Validity::Valid;
        match col {
            ColumnId::Pid => {
                out.bits = u64::from(r.key.pid);
                if out.want_text {
                    let _ = write!(out.text, "{}", r.key.pid);
                }
            }
            ColumnId::User => {
                out.bits = arc_bits(&r.user);
                out.arc = Some(Arc::clone(&r.user));
            }
            ColumnId::ImagePath => {
                // The executable path only, never a command line.
                if let Some(exe) = &r.exe {
                    out.bits = arc_bits(exe);
                    out.arc = Some(Arc::clone(exe));
                }
            }
            col => {
                let Some(value) = Metric::of_row(r, col) else {
                    out.bits = arc_bits(&r.name);
                    out.arc = Some(Arc::clone(&r.name));
                    return;
                };
                if !r.present {
                    out.validity = Validity::Missing;
                    out.tooltip = self.tips.get(Validity::Missing);
                    return;
                }
                out.bits = value.bits();
                if out.want_text {
                    value.write(&mut out.text, self.net);
                }
                if out.want_tooltip
                    && let Some(peak) = r.peak.as_ref().and_then(|p| Metric::of_peak(p, col))
                {
                    out.tooltip = Some(self.peak_tip(ix, col, peak));
                }
            }
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let r = self.row(ix);
        let icon = r
            .app_id
            .as_ref()
            .and_then(|app| self.icons.get(app))
            .map_or(NameIcon::None, |icon| NameIcon::File(Arc::clone(icon)));
        let w = &self.words;
        let (secondary, glyphs) = if r.previous_boot {
            (
                Some(w.previous_boot.clone()),
                smallvec![(IconName::RotateCcwClock, w.previous_boot.clone())],
            )
        } else if !r.present {
            (Some(w.not_running.clone()), SmallVec::new())
        } else {
            (None, SmallVec::new())
        };
        NameCell {
            icon,
            icon_size: IconSlot::Small,
            primary: SharedString::from(&r.name),
            count_run: None,
            secondary,
            glyphs,
            highlight_fffd: true,
            trailing: None,
        }
    }

    /// CPU and Memory show the system totals at the scrub time (averages over a range); Disk,
    /// Network and GPU show `-`.
    fn header_total(&self, col: ColumnId, out: &mut CellOut) -> bool {
        let totals = self.view.totals;
        let value = match col {
            ColumnId::Cpu => totals.map(|t| Metric::Percent(t.cpu_pct)),
            ColumnId::Memory => totals.filter(|t| t.mem_total_bytes > 0).map(|t| {
                Metric::Percent((t.mem_used_bytes as f64 / t.mem_total_bytes as f64 * 100.0) as f32)
            }),
            ColumnId::Disk | ColumnId::Network | ColumnId::Gpu => None,
            _ => return false,
        };
        let Some(value) = value else {
            out.bits = 0;
            out.validity = Validity::Missing;
            return true;
        };
        out.bits = value.bits();
        out.validity = Validity::Valid;
        if out.want_text {
            value.write(&mut out.text, self.net);
        }
        true
    }

    fn sort(&self) -> SortSpec {
        self.sort
    }

    fn selectable(&self, _: usize) -> bool {
        true
    }

    /// The History row menu (UI-PRC-031): the four copies, Show in live table (disabled when no
    /// selected row is running) and Delete from history. Copy needs the clicked column, so the
    /// keyboard menu leaves it out.
    fn context_menu(
        &mut self,
        ids: &[ProcessKey],
        clicked_col: Option<ColumnId>,
        menu: TlMenu,
        _: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let keys: SmallVec<[ProcessKey; 8]> = ids
            .iter()
            .copied()
            .filter(|k| self.index.contains_key(k))
            .collect();
        if keys.is_empty() {
            return menu;
        }
        let any_live = keys.iter().any(|&k| is_live(k, cx));
        self.captured = Some((keys, clicked_col));
        let mut menu = menu;
        if let Some(col) = clicked_col {
            menu = menu.item(tr(cx, "processes-menu-copy"), Box::new(CopyCell { col }));
        }
        menu = menu
            .item(tr(cx, "processes-menu-copy-details"), Box::new(CopyDetails))
            .item(
                tr(cx, "processes-menu-copy-all-metrics"),
                Box::new(CopyAllMetrics),
            )
            .item(
                tr(cx, "processes-menu-copy-rows-as-table"),
                Box::new(CopyRowsAsTable),
            )
            .separator();
        let show = tr(cx, "history-menu-show-live");
        menu = if any_live {
            menu.item(show, Box::new(HistoryShowInLiveTable))
        } else {
            menu.item_disabled(show, None)
        };
        menu.separator().danger_item(
            tr(cx, "history-menu-delete"),
            TlIcon::Trash,
            Box::new(DeleteFromHistory),
        )
    }

    fn on_toggle_expand(&mut self, _: ProcessKey, _: &mut Context<TlTreeTable<Self>>) {}

    /// Sorts the rows here and persists `[sort.history]`. Numeric columns start descending,
    /// text columns ascending.
    fn on_sort(&mut self, col: ColumnId, cx: &mut Context<TlTreeTable<Self>>) {
        self.sort = next_sort(&self.sort, col, self.column_spec(col).kind);
        self.rebuild();
        if cx.has_global::<TlSettings>() {
            let saved = SortState {
                column: self.sort.column.as_str().to_owned(),
                ascending: self.sort.ascending,
            };
            TlSettings::update(cx, |d| d.state.sort.history = saved);
        }
    }

    fn on_columns_changed(&mut self, layout: &ColumnLayout, cx: &mut Context<TlTreeTable<Self>>) {
        self.layout = layout.clone();
        if cx.has_global::<TlSettings>() {
            let state = layout.to_state();
            TlSettings::update(cx, |d| d.state.columns.history = state);
        }
    }

    fn on_visible_range(&mut self, _: &[ProcessKey], _: &mut Context<TlTreeTable<Self>>) {}

    /// Enter: remembers the row and lets `OpenProperties` go on to the page, which jumps to it.
    fn on_activate(&mut self, id: ProcessKey, cx: &mut Context<TlTreeTable<Self>>) {
        self.activated = Some(id);
        cx.propagate();
    }

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn empty_text(&self) -> Option<SharedString> {
        None
    }
}
