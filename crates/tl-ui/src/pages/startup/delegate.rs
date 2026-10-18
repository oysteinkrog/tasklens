//! The Startup Apps table delegate (UI-SYS-041, UI-SYS-042, UI-SYS-043, SYS-019 to SYS-021).
//!
//! `StartupDelegate` fills the seven columns of a startup row from the installed
//! `StartupSnapshot`. The engine sorts and filters (R-011); the delegate only maps rows to
//! entries and entries to cell text. Every label is translated once per strings version into
//! [`StartupWords`] (R-132) and handed to the table as an `Arc<str>`, so a frame formats
//! nothing. The display form of each Location (`$HOME` shown as `~`) is built once per snapshot.

use std::cell::RefCell;
use std::hash::BuildHasher as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, WeakEntity, Window, div, px,
};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, Impact, ImpactDetail, Snapshot, StartupEntry, StartupKind,
    StartupSnapshot, StartupStatus,
};
use tl_model::table::{StartupColumn, StartupRef, SysColumnId, SysNodeId, SysRow, SysSortSpec};
use tl_model::units;
use tl_model::validity::Validity;

use super::StartupPage;
use super::menu::{StartupMenuTargets, row_menu};
use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_ROW_NAME, TlTooltip};
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::pages::sys_status::{DotStyle, LabelTone, StatusLook, SysStatus, status_look};
use crate::pages::sys_table::SysDelegate;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};
use crate::theme::{TlTheme, current};

/// The seven columns in display order (UI-SYS-041).
pub const STARTUP_COLUMNS: [StartupColumn; 7] = [
    StartupColumn::Name,
    StartupColumn::Publisher,
    StartupColumn::Status,
    StartupColumn::Impact,
    StartupColumn::Type,
    StartupColumn::Location,
    StartupColumn::Command,
];

/// Default width of `c`, logical px at 100% (UI-SYS-041).
pub fn default_width(c: StartupColumn) -> f32 {
    match c {
        StartupColumn::Name => 220.,
        StartupColumn::Publisher => 180.,
        StartupColumn::Status => 90.,
        StartupColumn::Impact => 100.,
        StartupColumn::Type => 190.,
        StartupColumn::Location => 260.,
        StartupColumn::Command => 170.,
    }
}

/// Minimum width of `c`, logical px at 100% (UI-SYS-041). `SysColumnId::min_width` reads it.
pub fn min_width(c: StartupColumn) -> f32 {
    match c {
        StartupColumn::Name => 120.,
        StartupColumn::Status | StartupColumn::Impact => 80.,
        StartupColumn::Publisher
        | StartupColumn::Type
        | StartupColumn::Location
        | StartupColumn::Command => 60.,
    }
}

/// The Startup id of `c`.
pub const fn id(c: StartupColumn) -> SysColumnId {
    SysColumnId::Startup(c)
}

/// Default sort: Status ascending; the engine breaks ties by name (R-011).
pub const DEFAULT_SORT: SysSortSpec = SysSortSpec {
    column: id(StartupColumn::Status),
    ascending: true,
};

/// The UI status word of a startup status (SYS-020).
pub fn sys_status(s: StartupStatus) -> SysStatus {
    match s {
        StartupStatus::Enabled => SysStatus::Enabled,
        StartupStatus::Disabled => SysStatus::Disabled,
        StartupStatus::Masked => SysStatus::Masked,
        StartupStatus::NotInThisDesktop => SysStatus::NotInThisDesktop,
        StartupStatus::MissingProgram => SysStatus::MissingProgram,
    }
}

/// The Lucide icon of an entry with no icon of its own: `app-window` for autostart entries,
/// `cog` for user units, `file-code` for Plasma scripts.
pub fn fallback_icon(kind: StartupKind) -> IconName {
    match kind {
        StartupKind::AutostartUser
        | StartupKind::AutostartSystem
        | StartupKind::AutostartSystemOverridden => IconName::AppWindow,
        StartupKind::UserUnit => IconName::Cog,
        StartupKind::PlasmaPreStartup | StartupKind::PlasmaLogout => IconName::FileCode,
    }
}

/// `path` with a leading `home` shown as `~`, or `None` when it does not start with `home`.
pub fn home_tilde(path: &str, home: &str) -> Option<String> {
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return None;
    }
    let rest = path.strip_prefix(home)?;
    (rest.is_empty() || rest.starts_with('/')).then(|| format!("~{rest}"))
}

/// The text colour role of an impact (UI-SYS-041): High `status.warning`, Medium and Low
/// primary, None and Not measured secondary.
pub fn impact_tone(i: Impact) -> LabelTone {
    match i {
        Impact::High => LabelTone::Warning,
        Impact::Medium | Impact::Low => LabelTone::Primary,
        Impact::None | Impact::NotMeasured => LabelTone::Secondary,
    }
}

const COLUMN_KEYS: [&str; 7] = [
    "startup-column-name",
    "startup-column-publisher",
    "startup-column-status",
    "startup-column-impact",
    "startup-column-type",
    "startup-column-location",
    "startup-column-command",
];

const KIND_KEYS: [&str; 6] = [
    "startup-type-autostart-user",
    "startup-type-autostart-system",
    "startup-type-autostart-system-overridden",
    "startup-type-user-unit",
    "startup-type-plasma-pre-startup",
    "startup-type-plasma-logout",
];

const IMPACT_KEYS: [&str; 5] = [
    "startup-impact-high",
    "startup-impact-medium",
    "startup-impact-low",
    "startup-impact-none",
    "startup-impact-not-measured",
];

const STATUSES: [StartupStatus; 5] = [
    StartupStatus::Enabled,
    StartupStatus::Disabled,
    StartupStatus::Masked,
    StartupStatus::NotInThisDesktop,
    StartupStatus::MissingProgram,
];

fn kind_index(k: StartupKind) -> usize {
    match k {
        StartupKind::AutostartUser => 0,
        StartupKind::AutostartSystem => 1,
        StartupKind::AutostartSystemOverridden => 2,
        StartupKind::UserUnit => 3,
        StartupKind::PlasmaPreStartup => 4,
        StartupKind::PlasmaLogout => 5,
    }
}

fn impact_index(i: Impact) -> usize {
    match i {
        Impact::High => 0,
        Impact::Medium => 1,
        Impact::Low => 2,
        Impact::None => 3,
        Impact::NotMeasured => 4,
    }
}

fn status_index(s: StartupStatus) -> usize {
    match s {
        StartupStatus::Enabled => 0,
        StartupStatus::Disabled => 1,
        StartupStatus::Masked => 2,
        StartupStatus::NotInThisDesktop => 3,
        StartupStatus::MissingProgram => 4,
    }
}

/// The Startup Apps words, translated once per strings version.
#[derive(Clone, Debug)]
pub struct StartupWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    /// Header labels in [`STARTUP_COLUMNS`] order.
    pub columns: [SharedString; 7],
    /// Type labels by `StartupKind`.
    pub kinds: [Arc<str>; 6],
    /// Impact labels, High to Not measured.
    pub impacts: [Arc<str>; 5],
    /// Status labels by `StartupStatus` (the shared `sys-sys003-*` keys).
    pub statuses: [Arc<str>; 5],
    /// The Publisher of a user's own entry.
    pub user: Arc<str>,
    /// The Status label of an entry with an Enable or Disable in flight.
    pub updating: Arc<str>,
    pub activation: Template,
    pub started: Template,
    pub cpu: Template,
    pub fork_exec_only: SharedString,
    pub impact_none: SharedString,
    pub impact_not_measured: SharedString,
    /// The accessible row name "<Name>, <Status>, <Type>".
    pub row_name: Template,
}

impl StartupWords {
    /// Every word in the current language, or en-US when no strings are installed.
    pub fn new(cx: &App) -> Self {
        match cx.try_global::<Strings>() {
            Some(s) => Self::build(
                Some(s.strings_version),
                |key| tr(cx, key),
                |key, args| Template::new(cx, key, args),
            ),
            None => Self::en(),
        }
    }

    /// Every word in en-US.
    pub fn en() -> Self {
        Self::build(None, |key| tr_en(key, &[]), Template::en)
    }

    fn build(
        version: Option<u64>,
        text: impl Fn(&str) -> SharedString,
        template: impl Fn(&str, &[&str]) -> Template,
    ) -> Self {
        let arc = |key: &str| -> Arc<str> { Arc::from(&*text(key)) };
        Self {
            version,
            columns: COLUMN_KEYS.map(&text),
            kinds: KIND_KEYS.map(arc),
            impacts: IMPACT_KEYS.map(arc),
            statuses: STATUSES.map(|s| arc(sys_status(s).key())),
            user: arc("startup-publisher-user"),
            updating: arc("startup-status-updating"),
            activation: template("startup-tooltip-activation", &["ms"]),
            started: template("startup-tooltip-started", &["seconds"]),
            cpu: template("startup-tooltip-cpu", &["time"]),
            fork_exec_only: text("startup-tooltip-fork-exec-only"),
            impact_none: text("startup-tooltip-impact-none"),
            impact_not_measured: text("startup-tooltip-impact-not-measured"),
            row_name: template("startup-row-name", &["name", "status", "type"]),
        }
    }

    pub fn kind(&self, k: StartupKind) -> &Arc<str> {
        &self.kinds[kind_index(k)]
    }

    pub fn impact(&self, i: Impact) -> &Arc<str> {
        &self.impacts[impact_index(i)]
    }

    pub fn status(&self, s: StartupStatus) -> &Arc<str> {
        &self.statuses[status_index(s)]
    }
}

/// The Startup impact tooltip lines (UI-SYS-041, SYS-021). High, Medium and Low show the
/// activation time, the start offset and the CPU time (when known), then the fork/exec note
/// when `fork_exec_only`. None and Not measured show one sentence. A measured impact without
/// detail has no tooltip.
pub fn impact_tooltip_lines(
    impact: Impact,
    detail: Option<&ImpactDetail>,
    fork_exec_only: bool,
    w: &StartupWords,
) -> SmallVec<[String; 4]> {
    let mut lines = SmallVec::new();
    match impact {
        Impact::None => lines.push(w.impact_none.to_string()),
        Impact::NotMeasured => lines.push(w.impact_not_measured.to_string()),
        Impact::High | Impact::Medium | Impact::Low => {
            let Some(d) = detail else {
                return lines;
            };
            let mut ms = String::new();
            units::count(&mut ms, d.activation_ms);
            let mut line = String::new();
            let _ = w.activation.write(&mut line, &[&ms]);
            lines.push(line);
            let seconds = format!("{:.1}", d.started_after_login_ms as f64 / 1000.);
            let mut line = String::new();
            let _ = w.started.write(&mut line, &[&seconds]);
            lines.push(line);
            if let Some(ns) = d.cpu_nsec {
                let mut time = String::new();
                units::cpu_time(&mut time, ns / 1_000_000_000);
                let mut line = String::new();
                let _ = w.cpu.write(&mut line, &[&time]);
                lines.push(line);
            }
            if fork_exec_only {
                lines.push(w.fork_exec_only.to_string());
            }
        }
    }
    lines
}

/// The column specs in display order, with the labels of `w`.
pub fn startup_specs(w: &StartupWords) -> Vec<ColumnSpec<SysColumnId>> {
    STARTUP_COLUMNS
        .iter()
        .zip(&w.columns)
        .map(|(&c, label)| ColumnSpec {
            id: id(c),
            label: label.clone(),
            full_name: label.clone(),
            group: label.clone(),
            align: ColumnAlign::Left,
            kind: ColumnKind::Text,
            sortable: true,
            template: None,
            default_width: px(default_width(c)),
            tooltip: None,
            availability: Availability::Available,
            hideable: c != StartupColumn::Name,
        })
        .collect()
}

/// The table row id of `node`: a hash of its `StartupRef`, so a selection survives a rescan
/// (R-082). The `TreeTableDelegate` row id must be `Copy`.
pub fn row_key(node: &SysNodeId) -> u64 {
    FxBuildHasher.hash_one(node)
}

/// The hovered row's tooltip lines, shared between the delegate and [`StartupRowTip`].
#[derive(Default)]
struct RowTipState {
    /// (component version, row index) the lines were built for.
    key: Option<(u64, usize)>,
    lines: Vec<SharedString>,
}

/// The Name cell tooltip view: the full command, then the source file path.
pub struct StartupRowTip {
    state: Rc<RefCell<RowTipState>>,
    built: Option<((u64, usize), Entity<TlTooltip>)>,
}

impl Render for StartupRowTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.borrow();
        let fresh = match (&self.built, state.key) {
            (Some((built, _)), Some(key)) => *built == key,
            _ => false,
        };
        if !fresh && let Some(key) = state.key {
            let tip = TlTooltip::lines(state.lines.clone());
            self.built = Some((key, cx.new(|_| tip)));
        }
        div().children(self.built.as_ref().map(|(_, view)| view.clone()))
    }
}

/// The Startup Apps `TreeTableDelegate` and `SysDelegate` (UI-SYS-002).
pub struct StartupDelegate {
    snap: Option<Arc<StartupSnapshot>>,
    rows: Arc<[SysRow]>,
    /// Row index by [`row_key`].
    index: FxHashMap<u64, usize>,
    /// Display form of each entry's Location, by entry index.
    locations: Vec<Arc<str>>,
    query_generation: u64,
    header: ComponentHeader,
    home: Arc<str>,
    specs: Vec<ColumnSpec<SysColumnId>>,
    layout: ColumnLayout<SysColumnId>,
    sort: SysSortSpec,
    /// The sort the user picked in the header, for the page to send (`take_sort_request`).
    sort_request: Option<SysSortSpec>,
    words: StartupWords,
    theme: Arc<TlTheme>,
    version: u64,
    tip: Rc<RefCell<RowTipState>>,
    tip_view: Option<AnyView>,
    page: Option<WeakEntity<StartupPage>>,
    /// Entries with an Enable or Disable in flight (UI-SYS-042).
    updating: FxHashSet<StartupRef>,
}

/// The Status cell look of an entry with an Enable or Disable in flight: a filled info dot.
pub const UPDATING_LOOK: StatusLook = StatusLook {
    dot: DotStyle::Filled(LabelTone::Info),
    label: LabelTone::Primary,
};

impl StartupDelegate {
    /// An empty table. `home` is `$HOME`, shown as `~` in the Location column.
    pub fn new(home: Arc<str>, cx: &App) -> Self {
        let words = StartupWords::new(cx);
        let specs = startup_specs(&words);
        let order: Vec<SysColumnId> = STARTUP_COLUMNS.map(id).to_vec();
        Self {
            snap: None,
            rows: Arc::from([]),
            index: FxHashMap::default(),
            locations: Vec::new(),
            query_generation: 0,
            header: ComponentHeader::default(),
            home,
            layout: ColumnLayout::defaults(TableId::Startup, &specs, &order, &order),
            specs,
            sort: DEFAULT_SORT,
            sort_request: None,
            words,
            theme: current(cx).clone(),
            version: 0,
            tip: Rc::default(),
            tip_view: None,
            page: None,
            updating: FxHashSet::default(),
        }
    }

    /// Marks `entry` as having an Enable or Disable in flight, or clears the mark.
    pub(super) fn set_updating(&mut self, entry: &StartupRef, on: bool) {
        let changed = if on {
            self.updating.insert(entry.clone())
        } else {
            self.updating.remove(entry)
        };
        if changed {
            self.version += 1;
        }
    }

    /// True while `entry` shows "Updating…".
    pub fn is_updating(&self, entry: &StartupRef) -> bool {
        self.updating.contains(entry)
    }

    /// The page where a row menu stores the targets captured at open time.
    pub(super) fn set_page(&mut self, page: WeakEntity<StartupPage>) {
        self.page = Some(page);
    }

    /// Captures the current entries of `ids` in their supplied view order.
    pub(super) fn capture_menu_targets(
        &self,
        ids: &[u64],
        clicked_col: Option<SysColumnId>,
    ) -> StartupMenuTargets {
        let mut refs: SmallVec<[StartupRef; 1]> = SmallVec::new();
        let mut entries: SmallVec<[StartupEntry; 1]> = SmallVec::new();
        for id in ids {
            let Some(ix) = self.row_index(*id) else {
                continue;
            };
            if let (Some(r), Some(entry)) = (self.startup_ref(ix), self.entry(ix)) {
                refs.push(r.clone());
                entries.push(entry.clone());
            }
        }
        // The table API supplies the selection in view order but not the pressed row, so the
        // first row owns the cell action.
        let cell = clicked_col.and_then(|col| refs.first().map(|r| (r.clone(), col)));
        StartupMenuTargets {
            refs,
            entries,
            cell,
        }
    }

    /// `$HOME` from the environment, empty when unset.
    pub fn env_home() -> Arc<str> {
        Arc::from(std::env::var("HOME").unwrap_or_default())
    }

    /// `$HOME`, shown as `~` in the Location column.
    pub fn home(&self) -> &Arc<str> {
        &self.home
    }

    pub fn words(&self) -> &StartupWords {
        &self.words
    }

    /// The sort the page last sent; the header arrow follows it.
    pub fn set_sort(&mut self, sort: SysSortSpec) {
        self.sort = sort;
    }

    /// The sort the user picked since the last call, for the page to send to the engine.
    pub fn take_sort_request(&mut self) -> Option<SysSortSpec> {
        self.sort_request.take()
    }

    /// The `StartupRef` of row `ix`, for the page's actions.
    pub fn startup_ref(&self, ix: usize) -> Option<&StartupRef> {
        match &self.rows.get(ix)?.node {
            SysNodeId::Startup(r) => Some(r),
            _ => None,
        }
    }

    /// The entry of row `ix`.
    pub fn entry(&self, ix: usize) -> Option<&StartupEntry> {
        let record = self.rows.get(ix)?.record? as usize;
        self.snap.as_ref()?.entries.get(record)
    }

    fn location(&self, ix: usize) -> Option<&Arc<str>> {
        let record = self.rows.get(ix)?.record? as usize;
        self.locations.get(record)
    }

    fn stale(&self) -> bool {
        self.header.validity == Validity::Stale
    }

    /// The accessible row name "<Name>, <Status>, <Type>".
    pub fn accessible_name(&self, ix: usize) -> Option<String> {
        let e = self.entry(ix)?;
        let mut out = String::new();
        let _ = self.words.row_name.write(
            &mut out,
            &[
                &&*e.name,
                &&**self.words.status(e.status),
                &&**self.words.kind(e.kind),
            ],
        );
        Some(out)
    }

    /// The Name tooltip lines of row `ix`: the full command, then the source file path.
    pub fn name_tooltip_lines(&self, ix: usize) -> Option<[SharedString; 2]> {
        let e = self.entry(ix)?;
        Some([
            SharedString::from(e.command.clone()),
            SharedString::from(e.source_path.clone()),
        ])
    }

    /// The Startup impact tooltip of row `ix`, one line per `\n`.
    pub fn impact_tooltip(&self, ix: usize) -> Option<SharedString> {
        let e = self.entry(ix)?;
        let fork_exec_only = e.impact_detail.is_some_and(|d| d.fork_exec_only);
        let lines = impact_tooltip_lines(
            e.impact,
            e.impact_detail.as_ref(),
            fork_exec_only,
            &self.words,
        );
        (!lines.is_empty()).then(|| lines.join("\n").into())
    }

    /// Re-reads the words and theme when they changed. Returns true on a change.
    fn refresh_look(&mut self, cx: &App) -> bool {
        let mut changed = false;
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        if version != self.words.version {
            self.words = StartupWords::new(cx);
            self.specs = startup_specs(&self.words);
            changed = true;
        }
        let theme = current(cx);
        if !Arc::ptr_eq(theme, &self.theme) {
            self.theme = theme.clone();
            changed = true;
        }
        if changed {
            self.version += 1;
        }
        changed
    }
}

impl TreeTableDelegate for StartupDelegate {
    /// [`row_key`] of the row's `StartupRef`: kept across rescans (R-082).
    type RowId = u64;
    type Column = SysColumnId;
    type Sort = SysSortSpec;

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn row_id(&self, ix: usize) -> u64 {
        row_key(&self.rows[ix].node)
    }

    fn row_index(&self, id: u64) -> Option<usize> {
        self.index.get(&id).copied()
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
        if self.stale() {
            RowFlags {
                bits: RowFlags::STALE,
                ..RowFlags::default()
            }
        } else {
            RowFlags::default()
        }
    }

    fn table_version(&self) -> u64 {
        self.version
    }

    fn columns(&self) -> &ColumnLayout<SysColumnId> {
        &self.layout
    }

    fn column_spec(&self, col: SysColumnId) -> &ColumnSpec<SysColumnId> {
        self.specs
            .iter()
            .find(|s| s.id == col)
            .unwrap_or(&self.specs[0])
    }

    fn cell(&self, ix: usize, col: SysColumnId, out: &mut CellOut) {
        let (Some(e), SysColumnId::Startup(c)) = (self.entry(ix), col) else {
            return;
        };
        // Every value here is current; the default `WarmingUp` would draw the "…" glyph.
        out.validity = Validity::Valid;
        let theme = &self.theme;
        let w = &self.words;
        match c {
            StartupColumn::Name => out.arc = Some(e.name.clone()),
            StartupColumn::Publisher => match &e.publisher {
                // The engine writes "User" for the user's own entries (UI-SYS-042).
                Some(p) if &**p == "User" => {
                    out.arc = Some(w.user.clone());
                    out.text_color = Some(theme.text_secondary);
                }
                Some(p) => out.arc = Some(p.clone()),
                None => {}
            },
            StartupColumn::Status if self.updating.contains(&e.id) => {
                out.arc = Some(w.updating.clone());
            }
            StartupColumn::Status => {
                out.arc = Some(w.status(e.status).clone());
                if status_look(sys_status(e.status)).label == LabelTone::Secondary {
                    out.text_color = Some(theme.text_secondary);
                }
            }
            StartupColumn::Impact => {
                out.arc = Some(w.impact(e.impact).clone());
                out.text_color = match impact_tone(e.impact) {
                    LabelTone::Warning => Some(theme.status.warning),
                    LabelTone::Secondary => Some(theme.text_secondary),
                    _ => None,
                };
                if out.want_tooltip {
                    out.tooltip = self.impact_tooltip(ix);
                }
            }
            StartupColumn::Type => {
                out.arc = Some(w.kind(e.kind).clone());
                if e.kind == StartupKind::AutostartSystemOverridden {
                    out.text_color = Some(theme.text_secondary);
                }
            }
            StartupColumn::Location => {
                out.arc = self.location(ix).cloned();
                out.text_color = Some(theme.text_secondary);
            }
            StartupColumn::Command => out.arc = Some(e.command.clone()),
        }
        if e.overridden_by_user {
            out.text_color = Some(theme.text_secondary);
        }
    }

    fn name_cell(&self, ix: usize) -> NameCell {
        let (icon, primary) = match self.entry(ix) {
            Some(e) => (
                match &e.icon_path {
                    Some(path) => NameIcon::File(path.clone()),
                    None => NameIcon::Lucide(fallback_icon(e.kind)),
                },
                SharedString::from(e.name.clone()),
            ),
            None => (NameIcon::None, SharedString::default()),
        };
        NameCell {
            icon,
            icon_size: IconSlot::Small,
            primary,
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: true,
            trailing: None,
        }
    }

    fn name_column(&self) -> Option<SysColumnId> {
        Some(id(StartupColumn::Name))
    }

    fn header_total(&self, _: SysColumnId, _: &mut CellOut) -> bool {
        false
    }

    fn sort(&self) -> SysSortSpec {
        self.sort
    }

    fn selectable(&self, _: usize) -> bool {
        true
    }

    fn context_menu(
        &mut self,
        targets: &[u64],
        clicked_col: Option<SysColumnId>,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let captured = self.capture_menu_targets(targets, clicked_col);
        if captured.entries.is_empty() {
            return menu;
        }
        if let Some(page) = self.page.as_ref().and_then(WeakEntity::upgrade) {
            let stored = captured.clone();
            page.update(cx, |page, _| page.set_menu_targets(stored));
        }
        row_menu(menu, &captured, window, cx)
    }

    /// Drops the snapshot taken when the menu opened, so a dismissed menu cannot feed a later
    /// keyboard command.
    fn on_menu_closed(&mut self, cx: &mut Context<TlTreeTable<Self>>) {
        if let Some(page) = self.page.as_ref().and_then(WeakEntity::upgrade) {
            page.update(cx, |page, _| page.clear_menu_targets());
        }
    }

    fn on_toggle_expand(&mut self, _: u64, _: &mut Context<TlTreeTable<Self>>) {}

    /// A second click on the sort column flips the direction; another column sorts ascending.
    fn on_sort(&mut self, col: SysColumnId, cx: &mut Context<TlTreeTable<Self>>) {
        let ascending = if self.sort.column == col {
            !self.sort.ascending
        } else {
            true
        };
        self.sort = SysSortSpec {
            column: col,
            ascending,
        };
        self.sort_request = Some(self.sort);
        cx.notify();
    }

    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SysColumnId>,
        _: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
    }

    fn on_visible_range(&mut self, _: &[u64], _: &mut Context<TlTreeTable<Self>>) {}

    /// Enter, Alt+Enter and Ctrl+I reach the page as `OpenProperties`, which opens Startup
    /// Properties for the focus row.
    fn on_activate(&mut self, _: u64, cx: &mut Context<TlTreeTable<Self>>) {
        cx.propagate();
    }

    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        let view = self.tip_view.as_ref()?;
        let lines = self.name_tooltip_lines(ix)?;
        let key = (self.header.version, ix);
        let mut tip = self.tip.borrow_mut();
        if tip.key != Some(key) {
            tip.key = Some(key);
            tip.lines = lines.to_vec();
        }
        Some(view.clone())
    }

    fn empty_text(&self) -> Option<SharedString> {
        None
    }

    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }

    fn cell_tip_delay(&self) -> Duration {
        DELAY_ROW_NAME
    }

    fn cell_tip_content(&self, col: SysColumnId, tip: SharedString) -> TlTooltip {
        if col == id(StartupColumn::Impact) {
            TlTooltip::lines(tip.split('\n').map(|l| l.to_owned().into()).collect())
        } else {
            TlTooltip::text(tip)
        }
    }

    fn before_rows(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) {
        if self.tip_view.is_none() {
            let state = Rc::clone(&self.tip);
            self.tip_view = Some(cx.new(|_| StartupRowTip { state, built: None }).into());
        }
        if self.refresh_look(cx) {
            cx.notify();
        }
    }
}

impl SysDelegate for StartupDelegate {
    fn component_header<'a>(&self, snap: &'a Snapshot) -> &'a ComponentHeader {
        &snap.startup.header
    }

    fn rows_len(&self, snap: &Snapshot) -> usize {
        snap.startup.rows.len()
    }

    fn query_generation(&self, snap: &Snapshot) -> u64 {
        snap.startup.query_generation
    }

    fn install(&mut self, snap: &Arc<Snapshot>) {
        let startup = Arc::clone(&snap.startup);
        self.rows = Arc::clone(&startup.rows);
        self.index = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, r)| (row_key(&r.node), ix))
            .collect();
        self.locations = startup
            .entries
            .iter()
            .map(|e| match home_tilde(&e.location, &self.home) {
                Some(t) => Arc::from(t),
                None => e.location.clone(),
            })
            .collect();
        self.query_generation = startup.query_generation;
        self.header = startup.header;
        self.snap = Some(startup);
        self.tip.borrow_mut().key = None;
        self.version += 1;
    }

    fn mask(&self) -> ComponentMask {
        ComponentMask::STARTUP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_only_at_a_path_boundary() {
        assert_eq!(
            home_tilde("/home/u/.config/autostart", "/home/u").as_deref(),
            Some("~/.config/autostart")
        );
        assert_eq!(home_tilde("/home/u", "/home/u/").as_deref(), Some("~"));
        assert_eq!(home_tilde("/home/user2/x", "/home/u"), None);
        assert_eq!(home_tilde("/etc/xdg/autostart", "/home/u"), None);
        assert_eq!(home_tilde("/etc", ""), None);
    }
}
