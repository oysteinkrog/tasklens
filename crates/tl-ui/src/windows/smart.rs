//! "tasklens SMART Data" window: the drive's SMART summary and attributes (PERF-051, UI-PRF-030,
//! UI-FND-021, R-139).
//!
//! `PerfShowSmart` opens it, or focuses the open one, through a global handler: a page-level
//! handler would stop the action before the app-wide one. The data is the disk record's
//! `smart_data` in `snap.device_details.disks`. `None` means it was never read, so opening the
//! window sends `SmartRead { update: false }`; Refresh sends `update: true` and stays disabled
//! until the record's update time changes. A failed read puts the reason in `health.value`.
//!
//! A disk with `SmartKind::NeedsFallback` (no UDisks2 SMART, smartctl installed) skips the read
//! on open and shows "Read with smartctl (admin)" in Refresh's place (P3, R-088). It sends
//! `SmartReadAdmin` with the kernel name only; the engine builds the `pkexec smartctl` command
//! line. The button spins until the disk record changes.
//!
//! ATA drives get a flat sortable table, NVMe drives a two-column list in the engine's PERF-051
//! order. The table is rebuilt only when the `SmartData` pointer changes. A 1 s timer that lives
//! with the window moves the "Last updated" age. Esc and Ctrl+W close the window through the
//! frame.

use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fluent_bundle::FluentValue;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, AsyncApp, Context, Entity, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div, px, size,
};
use smallvec::SmallVec;
use tl_model::command::{Command, CommandError, DeviceCommand};
use tl_model::devices::DeviceIdentity;
use tl_model::ids::DeviceIdx;
use tl_model::snapshot::{DiskDetails, SmartAttr, SmartData, SmartKind, Snapshot};
use tl_model::units::{short_age, uptime};
use tl_model::validity::Validity;

use crate::app::EngineGlobal;
use crate::components::button::{TlButton, TlButtonKind, TlButtonSize};
use crate::components::kv_grid::{
    KvLayout, TlKeyValueGrid, TlKvBlock, TlKvRow, TlKvSection, TlValue, ValueTone,
};
use crate::components::menu::TlMenu;
use crate::components::toast::ToastKind;
use crate::components::window_frame::{TlSecondaryWindow, TlWindowKind, TlWindowSpec};
use crate::hub::{ComponentMask, GlobalHub, SnapshotChanged, SnapshotHub};
use crate::i18n::{Strings, tr, tr_args};
use crate::pages::performance::DeviceKey;
use crate::pages::performance::actions::{PerfShowSmart, PerfSmartReadSmartctl, PerfSmartRefresh};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableColumn, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TableSort, TlTreeTable,
    TreeTableDelegate,
};
use crate::text::style::{label_style, t_base, t_sm};
use crate::theme::{TlTheme, UiMetrics, current, metrics};
use crate::window::main::push_main_toast;
use crate::window::registry::{GlobalRegistry, WindowKey};

/// Default and minimum window size, × ui_scale.
pub const SIZE: (f32, f32) = (760., 520.);
pub const MIN_SIZE: (f32, f32) = (560., 360.);
/// NVMe list: Value column width and row height, × ui_scale.
const NVME_VALUE_W: f32 = 160.;
const NVME_ROW_H: f32 = 26.;
/// How often the "Last updated" age moves.
const TICK: Duration = Duration::from_secs(1);

/// The engine's NVMe row labels (PERF-051) and the locale key of each.
const NVME_LABELS: [(&str, &str); 15] = [
    ("Percent used", "perf-smart-nvme-percent-used"),
    ("Available spare", "perf-smart-nvme-available-spare"),
    ("Spare threshold", "perf-smart-nvme-spare-threshold"),
    ("Total data read", "perf-smart-nvme-total-read"),
    ("Total data written", "perf-smart-nvme-total-written"),
    ("Warning temp time", "perf-smart-nvme-warning-temp-time"),
    ("Critical temp time", "perf-smart-nvme-critical-temp-time"),
    ("Controller busy time", "perf-smart-nvme-busy-time"),
    ("Warning temperature", "perf-smart-nvme-warning-temp"),
    ("Critical temperature", "perf-smart-nvme-critical-temp"),
    ("Temperature sensors", "perf-smart-nvme-sensors"),
    ("Unsafe shutdowns", "perf-smart-nvme-unsafe-shutdowns"),
    ("Media errors", "perf-smart-nvme-media-errors"),
    ("Error log entries", "perf-smart-nvme-error-log"),
    ("Power cycles", "perf-smart-nvme-power-cycles"),
];

/// Registers the `PerfShowSmart` handler. `TasklensUi::init` calls it.
pub fn init(cx: &mut App) {
    // Deferred: the action can come from a menu while the page is mid-update.
    cx.on_action(|a: &PerfShowSmart, cx| {
        let key = a.key.clone();
        cx.defer(move |cx| show_smart(key, cx));
    });
    cx.on_action(|a: &PerfSmartRefresh, cx| {
        let key = a.key.clone();
        cx.defer(move |cx| refresh_open(&key, cx));
    });
    cx.on_action(|a: &PerfSmartReadSmartctl, cx| {
        let key = a.key.clone();
        cx.defer(move |cx| read_admin_open(&key, cx));
    });
}

/// Finds the disk of `key` in the catalog and opens its window.
fn show_smart(key: DeviceKey, cx: &mut App) {
    let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
        return;
    };
    let found = hub
        .read(cx)
        .current()
        .devices
        .entries
        .iter()
        .find(|e| DeviceKey::from_identity(&e.identity).as_ref() == Some(&key))
        .map(|e| e.idx);
    if let Some(idx) = found {
        open_smart(key, idx, cx);
    }
}

/// Opens the window of disk `idx`, or focuses the open one. A tool window "tasklens SMART Data"
/// keyed `WindowKey::Smart(idx)`.
pub fn open_smart(key: DeviceKey, idx: DeviceIdx, cx: &mut App) {
    let Some(hub) = cx.try_global::<GlobalHub>().map(|h| h.0.clone()) else {
        return;
    };
    let s = metrics(cx).ui_scale;
    let spec = TlWindowSpec::new(
        TlWindowKind::Tool { resizable: true },
        tr(cx, "perf-smart-title"),
        size(px(SIZE.0 * s), px(SIZE.1 * s)),
    )
    .min_size(size(px(MIN_SIZE.0 * s), px(MIN_SIZE.1 * s)))
    .key(WindowKey::Smart(idx));
    let open = move |cx: &mut App| {
        let handle = TlSecondaryWindow::open(cx, spec, move |window, cx| {
            cx.new(|cx| SmartWindow::new(key, idx, hub, window, cx))
        })?;
        if let Some(frame) = TlSecondaryWindow::<SmartWindow>::find(handle, cx) {
            // The title follows a language change.
            handle.update(cx, |_, window, cx| {
                frame.update(cx, |_, cx| {
                    cx.observe_global_in::<Strings>(window, |frame, window, cx| {
                        let title = tr(cx, "perf-smart-title");
                        if title != *frame.title() {
                            frame.set_title(title, window, cx);
                        }
                    })
                    .detach();
                });
            })?;
        }
        Ok(handle.into())
    };
    match cx.try_global::<GlobalRegistry>().map(|r| r.0.clone()) {
        Some(registry) => {
            registry.update(cx, |r, cx| r.open_or_focus(WindowKey::Smart(idx), cx, open));
        }
        None => {
            let _ = open(cx);
        }
    }
}

/// The open SMART window of `idx`, with its content.
pub fn find_smart(
    idx: DeviceIdx,
    cx: &App,
) -> Option<(
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    Entity<SmartWindow>,
)> {
    cx.windows().into_iter().find_map(|w| {
        let handle = w.downcast::<gpui_kit::base::Root>()?;
        let frame = TlSecondaryWindow::<SmartWindow>::find(handle, cx)?;
        let content = frame.read(cx).content().clone();
        (content.read(cx).idx == idx).then_some((handle, content))
    })
}

fn disk_details(snap: &Snapshot, idx: DeviceIdx) -> Option<&DiskDetails> {
    snap.device_details
        .disks
        .iter()
        .find(|(i, _)| *i == idx)
        .map(|(_, d)| d)
}

/// The kernel name of disk `idx` from the device catalog.
fn kernel_name(snap: &Snapshot, idx: DeviceIdx) -> Option<Arc<str>> {
    snap.devices
        .entries
        .iter()
        .find(|e| e.idx == idx)
        .and_then(|e| match &e.identity {
            DeviceIdentity::Disk { name, .. } => Some(name.clone()),
            _ => None,
        })
}

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// How an engine health or assessment text is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Ok,
    /// "Failed in the past" and NVMe "Warning: ...".
    Warning,
    Failing,
    Other,
}

impl Look {
    /// Classifies the engine's text: "OK", "Failing", "Failed in the past", "Warning: (list)".
    pub fn of(text: &str) -> Self {
        let t = text.trim();
        if t.eq_ignore_ascii_case("ok") {
            Self::Ok
        } else if t.eq_ignore_ascii_case("failing") {
            Self::Failing
        } else if t.eq_ignore_ascii_case("failed in the past") || t.starts_with("Warning") {
            Self::Warning
        } else {
            Self::Other
        }
    }

    pub fn color(self, theme: &TlTheme) -> Hsla {
        match self {
            Self::Ok | Self::Other => theme.text_primary,
            Self::Warning => theme.status.warning,
            Self::Failing => theme.status.danger,
        }
    }

    fn tone(self) -> Option<ValueTone> {
        match self {
            Self::Ok => Some(ValueTone::Success),
            Self::Warning => Some(ValueTone::Warning),
            Self::Failing => Some(ValueTone::Danger),
            Self::Other => None,
        }
    }
}

/// The window's texts, translated when the strings change.
struct Words {
    last_updated: SharedString,
    powered_on: SharedString,
    health: SharedString,
    selftest: SharedString,
    refresh: SharedString,
    smartctl: SharedString,
    ok: SharedString,
    failing: SharedString,
    failed_past: SharedString,
    unknown: SharedString,
    prefail: SharedString,
    oldage: SharedString,
    online: SharedString,
    offline: SharedString,
    failed: SharedString,
    attribute: SharedString,
    value: SharedString,
    headers: [SharedString; 9],
    nvme: [SharedString; 15],
}

impl Words {
    fn new(cx: &App) -> Self {
        let t = |key: &str| tr(cx, key);
        Self {
            last_updated: t("perf-smart-last-updated"),
            powered_on: t("perf-smart-powered-on"),
            health: t("perf-smart-health"),
            selftest: t("perf-smart-selftest"),
            refresh: t("perf-smart-refresh"),
            smartctl: t("perf-smart-smartctl"),
            ok: t("perf-smart-ok"),
            failing: t("perf-smart-failing"),
            failed_past: t("perf-smart-failed-past"),
            unknown: t("perf-smart-unknown"),
            prefail: t("perf-smart-prefail"),
            oldage: t("perf-smart-oldage"),
            online: t("perf-smart-online"),
            offline: t("perf-smart-offline"),
            failed: t("perf-smart-failed"),
            attribute: t("perf-smart-col-attribute"),
            value: t("perf-smart-col-value"),
            headers: [
                t("perf-smart-col-id"),
                t("perf-smart-col-attribute"),
                t("perf-smart-col-value"),
                t("perf-smart-col-normalized"),
                t("perf-smart-col-threshold"),
                t("perf-smart-col-worst"),
                t("perf-smart-col-type"),
                t("perf-smart-col-updates"),
                t("perf-smart-col-assessment"),
            ],
            nvme: NVME_LABELS.map(|(_, key)| t(key)),
        }
    }

    /// The word for an engine health or assessment text. Unknown texts show as given.
    fn status(&self, text: &str) -> SharedString {
        match Look::of(text) {
            Look::Ok => self.ok.clone(),
            Look::Failing => self.failing.clone(),
            Look::Warning if text.trim().eq_ignore_ascii_case("failed in the past") => {
                self.failed_past.clone()
            }
            _ => SharedString::from(text.to_owned()),
        }
    }

    /// The label of an NVMe row; a label the engine sends that is not in PERF-051 shows as given.
    fn nvme_label(&self, label: &str) -> SharedString {
        NVME_LABELS
            .iter()
            .position(|(en, _)| *en == label)
            .map_or_else(|| label.to_owned().into(), |i| self.nvme[i].clone())
    }
}

// ---- The ATA table ----

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SmartColumn {
    Id,
    Attribute,
    Value,
    Normalized,
    Threshold,
    Worst,
    Type,
    Updates,
    Assessment,
}

impl SmartColumn {
    pub const ALL: [SmartColumn; 9] = [
        Self::Id,
        Self::Attribute,
        Self::Value,
        Self::Normalized,
        Self::Threshold,
        Self::Worst,
        Self::Type,
        Self::Updates,
        Self::Assessment,
    ];

    /// Width at 100% and alignment. The Attribute column takes the width the others leave.
    fn meta(self) -> (f32, ColumnAlign) {
        use ColumnAlign::{Left, Right};
        match self {
            Self::Id => (40., Right),
            Self::Attribute => (210., Left),
            Self::Value => (80., Right),
            Self::Normalized => (90., Right),
            Self::Threshold => (80., Right),
            Self::Worst => (70., Right),
            Self::Type => (80., Left),
            Self::Updates => (80., Left),
            Self::Assessment => (110., Left),
        }
    }
}

impl TableColumn for SmartColumn {
    fn key(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Attribute => "attribute",
            Self::Value => "value",
            Self::Normalized => "normalized",
            Self::Threshold => "threshold",
            Self::Worst => "worst",
            Self::Type => "type",
            Self::Updates => "updates",
            Self::Assessment => "assessment",
        }
    }

    /// The layout is never saved.
    fn parse(_: TableId, _: &str) -> Option<Self> {
        None
    }

    fn pinned_first(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SmartSort {
    pub column: SmartColumn,
    pub ascending: bool,
}

impl TableSort<SmartColumn> for SmartSort {
    fn column(&self) -> SmartColumn {
        self.column
    }
    fn ascending(&self) -> bool {
        self.ascending
    }
}

fn compare(a: &SmartAttr, b: &SmartAttr, col: SmartColumn) -> Ordering {
    match col {
        SmartColumn::Id => a.id.cmp(&b.id),
        SmartColumn::Attribute => a.name.cmp(&b.name),
        SmartColumn::Value => a.value.cmp(&b.value),
        SmartColumn::Normalized => a.normalized.cmp(&b.normalized),
        SmartColumn::Threshold => a.threshold.cmp(&b.threshold),
        SmartColumn::Worst => a.worst.cmp(&b.worst),
        SmartColumn::Type => a.prefail.cmp(&b.prefail),
        SmartColumn::Updates => a.online.cmp(&b.online),
        SmartColumn::Assessment => a.assessment.cmp(&b.assessment),
    }
}

fn specs(words: &Words) -> Vec<ColumnSpec<SmartColumn>> {
    SmartColumn::ALL
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (width, align) = id.meta();
            let label = words.headers[i].clone();
            ColumnSpec {
                id,
                full_name: label.clone(),
                group: label.clone(),
                label,
                align,
                kind: match align {
                    ColumnAlign::Left => ColumnKind::Text,
                    ColumnAlign::Right => ColumnKind::Numeric,
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

/// The ATA attribute rows, their columns and sort.
pub struct SmartAtaDelegate {
    rows: Arc<[SmartAttr]>,
    /// Row indexes in sort order.
    order: Vec<u32>,
    sort: SmartSort,
    version: u64,
    specs: Vec<ColumnSpec<SmartColumn>>,
    layout: ColumnLayout<SmartColumn>,
    words: Rc<Words>,
    theme: Arc<TlTheme>,
}

impl SmartAtaDelegate {
    fn new(words: Rc<Words>, theme: Arc<TlTheme>) -> Self {
        let specs = specs(&words);
        let layout = ColumnLayout::defaults(
            TableId::Processes,
            &specs,
            &SmartColumn::ALL,
            &SmartColumn::ALL,
        );
        Self {
            rows: Arc::from([]),
            order: Vec::new(),
            // Click sorts descending first (E18-A2); the table starts in attribute order.
            sort: SmartSort {
                column: SmartColumn::Id,
                ascending: true,
            },
            version: 0,
            specs,
            layout,
            words,
            theme,
        }
    }

    fn set_rows(&mut self, rows: Arc<[SmartAttr]>) {
        self.rows = rows;
        self.resort();
    }

    fn resort(&mut self) {
        let rows = &self.rows;
        let SmartSort { column, ascending } = self.sort;
        self.order.clear();
        self.order.extend(0..rows.len() as u32);
        self.order.sort_by(|&a, &b| {
            let o = compare(&rows[a as usize], &rows[b as usize], column);
            if ascending { o } else { o.reverse() }
        });
        self.version += 1;
    }

    fn row(&self, ix: usize) -> &SmartAttr {
        &self.rows[self.order[ix] as usize]
    }

    /// The attribute ids in view order.
    pub fn ids(&self) -> Vec<u8> {
        (0..self.order.len()).map(|i| self.row(i).id).collect()
    }

    /// The text and colour of one cell.
    pub fn cell_of(&self, ix: usize, col: SmartColumn) -> (String, Option<Hsla>) {
        let mut out = CellOut {
            want_text: true,
            ..CellOut::default()
        };
        self.cell(ix, col, &mut out);
        (out.text, out.text_color)
    }

    pub fn sort_state(&self) -> SmartSort {
        self.sort
    }
}

impl TreeTableDelegate for SmartAtaDelegate {
    type RowId = u8;
    type Column = SmartColumn;
    type Sort = SmartSort;

    fn row_count(&self) -> usize {
        self.order.len()
    }
    fn row_id(&self, ix: usize) -> u8 {
        self.row(ix).id
    }
    fn row_index(&self, id: u8) -> Option<usize> {
        (0..self.order.len()).find(|&i| self.row(i).id == id)
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
    fn columns(&self) -> &ColumnLayout<SmartColumn> {
        &self.layout
    }
    fn column_spec(&self, id: SmartColumn) -> &ColumnSpec<SmartColumn> {
        let i = SmartColumn::ALL.iter().position(|c| *c == id).unwrap_or(0);
        &self.specs[i]
    }

    fn cell(&self, ix: usize, col: SmartColumn, out: &mut CellOut) {
        let r = self.row(ix);
        out.bits = match col {
            SmartColumn::Id => u64::from(r.id),
            SmartColumn::Attribute => (r.name.as_ptr() as u64) ^ ((r.name.len() as u64) << 48),
            SmartColumn::Value => r.value as u64,
            SmartColumn::Normalized => r.normalized as u32 as u64,
            SmartColumn::Threshold => r.threshold as u32 as u64,
            SmartColumn::Worst => r.worst as u32 as u64,
            SmartColumn::Type => u64::from(r.prefail),
            SmartColumn::Updates => u64::from(r.online),
            SmartColumn::Assessment => {
                (r.assessment.as_ptr() as u64) ^ ((r.assessment.len() as u64) << 48)
            }
        };
        out.validity = Validity::Valid;
        if col == SmartColumn::Assessment {
            out.text_color = Some(Look::of(&r.assessment).color(&self.theme));
        }
        if !out.want_text {
            return;
        }
        use std::fmt::Write as _;
        let w = &self.words;
        let _ = match col {
            SmartColumn::Id => write!(out.text, "{}", r.id),
            SmartColumn::Attribute => write!(out.text, "{}", r.name),
            SmartColumn::Value => write!(out.text, "{}", r.value),
            SmartColumn::Normalized => write!(out.text, "{}", r.normalized),
            SmartColumn::Threshold => write!(out.text, "{}", r.threshold),
            SmartColumn::Worst => write!(out.text, "{}", r.worst),
            SmartColumn::Type => {
                out.text
                    .push_str(if r.prefail { &w.prefail } else { &w.oldage });
                Ok(())
            }
            SmartColumn::Updates => {
                out.text
                    .push_str(if r.online { &w.online } else { &w.offline });
                Ok(())
            }
            SmartColumn::Assessment => {
                out.text.push_str(&w.status(&r.assessment));
                Ok(())
            }
        };
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::from(self.row(ix).name.clone()),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: false,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<SmartColumn> {
        Some(SmartColumn::Attribute)
    }

    fn header_total(&self, _: SmartColumn, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SmartSort {
        self.sort
    }
    fn selectable(&self, _: usize) -> bool {
        false
    }

    fn context_menu(
        &mut self,
        _: &[u8],
        _: Option<SmartColumn>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
    }

    fn empty_text(&self) -> Option<SharedString> {
        None
    }

    fn on_toggle_expand(&mut self, _: u8, _: &mut Context<TlTreeTable<Self>>) {}

    /// A new column sorts descending first; the same column flips.
    fn on_sort(&mut self, col: SmartColumn, _: &mut Context<TlTreeTable<Self>>) {
        self.sort = SmartSort {
            column: col,
            ascending: self.sort.column == col && !self.sort.ascending,
        };
        self.resort();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SmartColumn>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u8], _: &mut Context<TlTreeTable<Self>>) {}

    fn on_activate(&mut self, _: u8, _: &mut Context<TlTreeTable<Self>>) {}

    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn columns_fixed(&self) -> bool {
        true
    }
}

// ---- The window ----

/// The window content. The frame around it is `TlSecondaryWindow`.
pub struct SmartWindow {
    idx: DeviceIdx,
    key: DeviceKey,
    /// The kernel name, as the engine addresses the disk.
    disk: Arc<str>,
    data: Option<Arc<SmartData>>,
    /// The reason of a failed read.
    error: Option<SharedString>,
    refreshing: bool,
    /// The update time Refresh was clicked at; the button re-enables when it changes.
    refresh_from: Option<i64>,
    /// No UDisks2 SMART, smartctl installed: the smartctl button replaces Refresh.
    fallback: bool,
    reading_admin: bool,
    /// The record `SmartReadAdmin` was sent at; the button stops spinning when it changes.
    admin_from: Option<Arc<SmartData>>,
    /// The last record seen, in any validity.
    seen: Option<Arc<SmartData>>,
    table: Entity<TlTreeTable<SmartAtaDelegate>>,
    nvme: Vec<(SharedString, SharedString)>,
    words: Rc<Words>,
    theme: Arc<TlTheme>,
    _subs: SmallVec<[Subscription; 3]>,
}

impl SmartWindow {
    fn new(
        key: DeviceKey,
        idx: DeviceIdx,
        hub: Entity<SnapshotHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let words = Rc::new(Words::new(cx));
        let theme = current(cx).clone();
        let delegate = SmartAtaDelegate::new(words.clone(), theme.clone());
        let table = cx.new(|cx| TlTreeTable::new(delegate, window, cx));
        let mut subs = SmallVec::new();
        subs.push(cx.subscribe(&hub, |this, hub, ev: &SnapshotChanged, cx| {
            if ev.mask.contains(ComponentMask::DEVICE_DETAILS) {
                let snap = hub.read(cx).current().clone();
                if this.sync(&snap, cx) {
                    cx.notify();
                }
            }
        }));
        subs.push(cx.observe_global::<Strings>(|this, cx| {
            this.words = Rc::new(Words::new(cx));
            this.rebuild(cx);
            cx.notify();
        }));
        // The age moves once a second while the window lives.
        let tick = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        tick.detach();
        let disk = kernel_name(hub.read(cx).current(), idx).unwrap_or_else(|| Arc::from(""));
        let mut this = Self {
            idx,
            key,
            disk,
            data: None,
            error: None,
            refreshing: false,
            refresh_from: None,
            fallback: false,
            reading_admin: false,
            admin_from: None,
            seen: None,
            table,
            nvme: Vec::new(),
            words,
            theme,
            _subs: subs,
        };
        let snap = hub.read(cx).current().clone();
        this.sync(&snap, cx);
        // `None` means never read. UDisks2 cannot read a fallback disk.
        if !this.fallback
            && this.data.is_none()
            && this.error.is_none()
            && !this.record_exists(&snap)
        {
            this.send(
                DeviceCommand::SmartRead {
                    disk: this.disk.clone(),
                    update: false,
                },
                cx,
            );
        }
        this
    }

    fn record_exists(&self, snap: &Snapshot) -> bool {
        disk_details(snap, self.idx).is_some_and(|d| d.smart_data.is_some())
    }

    pub fn idx(&self) -> DeviceIdx {
        self.idx
    }

    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    pub fn refreshing(&self) -> bool {
        self.refreshing
    }

    /// The disk needs the smartctl read.
    pub fn fallback(&self) -> bool {
        self.fallback
    }

    /// A `SmartReadAdmin` was sent and the record has not changed yet.
    pub fn reading_admin(&self) -> bool {
        self.reading_admin
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn data(&self) -> Option<&Arc<SmartData>> {
        self.data.as_ref()
    }

    pub fn table(&self) -> &Entity<TlTreeTable<SmartAtaDelegate>> {
        &self.table
    }

    /// The NVMe list as drawn: translated label and value, in order.
    pub fn nvme_rows(&self) -> &[(SharedString, SharedString)] {
        &self.nvme
    }

    /// Takes the disk record's SMART state. True when something drawn changed.
    fn sync(&mut self, snap: &Snapshot, cx: &mut Context<Self>) -> bool {
        let Some(details) = disk_details(snap, self.idx) else {
            return false;
        };
        let fallback = details.smart == SmartKind::NeedsFallback;
        let mut changed = fallback != self.fallback;
        self.fallback = fallback;
        let Some(field) = details.smart_data.as_ref() else {
            return changed;
        };
        self.seen = Some(field.value.clone());
        if self.reading_admin {
            // The read is still running, or the record is the one from before the click.
            let old = self
                .admin_from
                .as_ref()
                .is_some_and(|d| Arc::ptr_eq(d, &field.value));
            if old || field.validity == Validity::WarmingUp {
                return changed;
            }
            self.reading_admin = false;
            self.admin_from = None;
            changed = true;
        }
        match field.validity {
            Validity::Failed => {
                let reason = field.value.health.value.clone();
                self.error = Some(SharedString::from(reason));
                self.refreshing = false;
                true
            }
            Validity::WarmingUp => changed,
            _ => {
                let new = &field.value;
                let new_data = self.error.take().is_some()
                    || self.data.as_ref().is_none_or(|d| !Arc::ptr_eq(d, new));
                if !new_data {
                    return changed;
                }
                if self.refreshing && self.refresh_from != Some(new.updated_s) {
                    self.refreshing = false;
                }
                self.data = Some(new.clone());
                self.rebuild(cx);
                true
            }
        }
    }

    /// Builds the table rows and the NVMe list from `data` with the current words.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let words = self.words.clone();
        self.nvme = self.data.as_ref().map_or_else(Vec::new, |d| {
            d.nvme
                .iter()
                .map(|(k, v)| (words.nvme_label(k), SharedString::from(v.clone())))
                .collect()
        });
        let rows = self
            .data
            .as_ref()
            .map_or_else(|| Arc::from([]), |d| d.ata.clone());
        self.table.update(cx, |t, cx| {
            let d = t.delegate_mut();
            d.specs = specs(&words);
            d.words = words;
            d.set_rows(rows);
            t.on_new_version(cx);
        });
    }

    /// Sends a SMART command. Returns false when it was not accepted. A full queue toasts for a
    /// click, not for the read on open.
    fn send(&mut self, cmd: DeviceCommand, cx: &mut Context<Self>) -> bool {
        let Some(engine) = cx.try_global::<EngineGlobal>().map(|e| e.0.clone()) else {
            return false;
        };
        let clicked = !matches!(cmd, DeviceCommand::SmartRead { update: false, .. });
        match engine.send(Command::Device(cmd)) {
            Ok(()) => true,
            Err(CommandError::Busy) => {
                if clicked {
                    let text = tr(cx, "perf-smart-refresh-busy");
                    push_main_toast(cx, ToastKind::Warning, text, None, None);
                }
                false
            }
            Err(CommandError::EngineStopped) => false,
        }
    }

    /// Refresh: one `SmartRead { update: true }`; the button stays disabled until the update
    /// time changes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        let cmd = DeviceCommand::SmartRead {
            disk: self.disk.clone(),
            update: true,
        };
        if self.send(cmd, cx) {
            self.refreshing = true;
            self.refresh_from = self.data.as_ref().map(|d| d.updated_s);
            self.error = None;
            cx.notify();
        }
    }

    /// "Read with smartctl (admin)": one `SmartReadAdmin` that carries only the kernel name. The
    /// button spins until the record changes.
    pub fn read_admin(&mut self, cx: &mut Context<Self>) {
        if self.reading_admin || !self.fallback {
            return;
        }
        let cmd = DeviceCommand::SmartReadAdmin {
            disk: self.disk.clone(),
        };
        if self.send(cmd, cx) {
            self.reading_admin = true;
            self.admin_from = self.seen.clone();
            self.error = None;
            cx.notify();
        }
    }

    fn header_grid(&self, cx: &App) -> TlKeyValueGrid {
        let w = &self.words;
        let missing = |cx_validity| TlValue {
            validity: cx_validity,
            ..TlValue::new("")
        };
        let (updated, powered, health, selftest) = match &self.data {
            Some(d) => {
                let age = {
                    let mut s = String::new();
                    short_age(&mut s, (now_s() - d.updated_s).max(0) as u64);
                    s
                };
                let ago = tr_args(cx, "perf-smart-ago", &[("age", FluentValue::from(age))]);
                let powered = if d.power_on_s.validity == Validity::Valid {
                    let mut s = String::new();
                    uptime(&mut s, d.power_on_s.value);
                    TlValue::new(s)
                } else {
                    missing(d.power_on_s.validity)
                };
                let health = {
                    let text = &d.health.value;
                    if d.health.validity == Validity::Valid && !text.trim().is_empty() {
                        let look = Look::of(text);
                        TlValue {
                            tone: look.tone(),
                            ..TlValue::new(w.status(text))
                        }
                    } else {
                        TlValue::new(w.unknown.clone())
                    }
                };
                let selftest = if d.selftest.validity == Validity::Valid {
                    TlValue::new(SharedString::from(d.selftest.value.clone()))
                } else {
                    missing(d.selftest.validity)
                };
                (TlValue::new(ago), powered, health, selftest)
            }
            None => (
                missing(Validity::WarmingUp),
                missing(Validity::WarmingUp),
                TlValue::new(w.unknown.clone()),
                missing(Validity::WarmingUp),
            ),
        };
        let row = |label: &SharedString, value: TlValue| TlKvRow {
            label: label.clone(),
            value,
        };
        let section = TlKvSection {
            title: None,
            blocks: vec![TlKvBlock {
                heading: None,
                indent: None,
                rows: vec![
                    row(&w.last_updated, updated),
                    row(&w.powered_on, powered),
                    row(&w.health, health),
                    row(&w.selftest, selftest),
                ],
            }],
        };
        TlKeyValueGrid::new("SmartHeader", KvLayout::Stacked)
            .columns(4)
            .sections(&[section])
    }
}

impl Render for SmartWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = current(cx).clone();
        if !Arc::ptr_eq(&theme, &self.theme) {
            self.theme = theme.clone();
            self.table.update(cx, |t, cx| {
                t.delegate_mut().theme = theme.clone();
                t.on_new_version(cx);
            });
        }
        let m = *metrics(cx);
        let s = m.ui_scale;
        let this = cx.entity().downgrade();
        let (button, selector) = if self.fallback {
            let b = TlButton::new("SmartReadSmartctl", TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .label(self.words.smartctl.clone())
                .loading(self.reading_admin)
                .disabled(self.reading_admin)
                .on_click(move |_, window, cx| {
                    if let Some(key) = this.upgrade().map(|w| w.read(cx).key.clone()) {
                        window.dispatch_action(Box::new(PerfSmartReadSmartctl { key }), cx);
                    }
                });
            (b, "SmartReadSmartctl")
        } else {
            let b = TlButton::new("SmartRefresh", TlButtonKind::Secondary)
                .size(TlButtonSize::Sm)
                .label(self.words.refresh.clone())
                .loading(self.refreshing)
                .disabled(self.refreshing)
                .on_click(move |_, window, cx| {
                    if let Some(key) = this.upgrade().map(|w| w.read(cx).key.clone()) {
                        window.dispatch_action(Box::new(PerfSmartRefresh { key }), cx);
                    }
                });
            (b, "SmartRefresh")
        };
        let header = div()
            .flex_none()
            .flex()
            .items_start()
            .gap(m.sp[3])
            .p(m.sp[3])
            .child(div().flex_1().min_w_0().child(self.header_grid(cx)))
            .child(
                div()
                    .debug_selector(move || selector.into())
                    .flex_none()
                    .child(button),
            );
        let body: AnyElement = if let Some(reason) = &self.error {
            self.render_failure(reason, &theme, &m)
        } else if self.data.as_ref().is_some_and(|d| !d.nvme.is_empty()) {
            self.render_nvme(&theme, &m, s)
        } else if self.fallback && self.data.is_none() {
            // Nothing read yet: no table until smartctl answers.
            div().flex_1().into_any_element()
        } else {
            div()
                .id("SmartTable")
                .debug_selector(|| "SmartTable".into())
                .flex_1()
                .min_h_0()
                .child(self.table.clone())
                .into_any_element()
        };
        div()
            .id("SmartWindow")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.window_bg)
            .child(header)
            .child(body)
    }
}

impl SmartWindow {
    fn render_failure(&self, reason: &SharedString, theme: &TlTheme, m: &UiMetrics) -> AnyElement {
        let mut title = div().text_color(theme.text_primary);
        *title.text_style() = label_style(m, m.text.md, gpui_kit::FontWeight::MEDIUM);
        let mut detail = div().text_color(theme.text_secondary);
        *detail.text_style() = t_sm(m);
        div()
            .id("SmartFailure")
            .debug_selector(|| "SmartFailure".into())
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(m.sp[2])
            .child(title.child(self.words.failed.clone()))
            .child(detail.child(reason.clone()))
            .into_any_element()
    }

    fn render_nvme(&self, theme: &TlTheme, m: &UiMetrics, s: f32) -> AnyElement {
        let row_h = px(NVME_ROW_H * s);
        let mut head_label = div().flex_1().min_w_0().text_color(theme.text_secondary);
        *head_label.text_style() = t_sm(m);
        let mut head_value = div()
            .flex_none()
            .w(px(NVME_VALUE_W * s))
            .flex()
            .justify_end()
            .text_color(theme.text_secondary);
        *head_value.text_style() = t_sm(m);
        let head = div()
            .flex_none()
            .flex()
            .items_center()
            .h(row_h)
            .px(m.sp[3])
            .border_b_1()
            .border_color(theme.divider)
            .child(head_label.child(self.words.attribute.clone()))
            .child(head_value.child(self.words.value.clone()));
        let rows = self.nvme.iter().enumerate().map(|(i, (label, value))| {
            let mut l = div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(theme.text_primary);
            *l.text_style() = t_base(m);
            let mut v = div()
                .flex_none()
                .w(px(NVME_VALUE_W * s))
                .flex()
                .justify_end()
                .text_color(theme.text_primary);
            *v.text_style() = t_base(m);
            div()
                .debug_selector(move || format!("SmartNvmeRow-{i}"))
                .flex_none()
                .flex()
                .items_center()
                .h(row_h)
                .px(m.sp[3])
                .child(l.child(label.clone()))
                .child(v.child(value.clone()))
        });
        div()
            .id("SmartNvme")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(head)
            .child(
                div()
                    .id("SmartNvmeRows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }
}

/// Handles the page's `PerfSmartRefresh`: sends the read through the open window.
pub fn refresh_open(key: &DeviceKey, cx: &mut App) {
    with_open(key, cx, SmartWindow::refresh);
}

/// Handles `PerfSmartReadSmartctl`: sends the smartctl read through the open window.
pub fn read_admin_open(key: &DeviceKey, cx: &mut App) {
    with_open(key, cx, SmartWindow::read_admin);
}

/// Runs `f` on the open window of `key`.
fn with_open(key: &DeviceKey, cx: &mut App, f: fn(&mut SmartWindow, &mut Context<SmartWindow>)) {
    let windows: Vec<_> = cx
        .windows()
        .into_iter()
        .filter_map(|w| w.downcast::<gpui_kit::base::Root>())
        .collect();
    for handle in windows {
        if let Some(frame) = TlSecondaryWindow::<SmartWindow>::find(handle, cx) {
            let content = frame.read(cx).content().clone();
            if content.read(cx).key == *key {
                content.update(cx, f);
            }
        }
    }
}
