//! Services table rows, columns, expansion and persistence (UI-SYS-002, UI-SYS-013).
//!
//! Child process rows (UI-SYS-016) paint through the process table's `ProcessRowCells`, with the
//! Services columns mapped by [`child_column`]. Each child finds its record by `ProcessKey` in
//! the installed snapshot, so a key that left the process snapshot draws as exited.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::hash::BuildHasher as _;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::{
    AnyView, AnyWindowHandle, App, AppContext as _, Context, Entity, IntoElement,
    ParentElement as _, Render, SharedString, WeakEntity, Window, div, px,
};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::demand::{ColumnSet, Demand};
use tl_model::health::ManagerState;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::{ProcFlags, ProcState, ProcessRecord};
use tl_model::settings::{Settings, SortState, UiState};
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ProcessSnapshot, ServiceBackend, ServiceStatus,
    ServicesSnapshot, Snapshot, StartupType, UnitRecord,
};
use tl_model::table::{
    Manager, ServiceColumn, SysColumnId, SysNodeId, SysRow, SysSortSpec, TableRow, Total, UnitRef,
    UnitType,
};
use tl_model::time::{BootId, now_boot_ns};
use tl_model::units;
use tl_model::units::LocalDateTime;
use tl_model::validity::Validity;

use crate::components::menu::TlMenu;
use crate::components::tooltip::{DELAY_ROW_NAME, TlTooltip};
use crate::demand::GlobalDemand;
use crate::i18n::{Strings, tr, tr_en};
use crate::pages::performance::details::Template;
use crate::pages::processes::cells::{CellCtx, CellWords, GroupMembers, PaintState, RowRef};
use crate::pages::processes::delegate::{ProcessRowCells, ValidityTips, live_tip};
use crate::pages::processes::name_cell::{self, NameCellData, NameWords, StateGlyph};
use crate::pages::processes::tooltip::{RowTipState as ProcTipState, RowTipWords, RowTooltip};
use crate::pages::sys_status::{DotStyle, LabelTone, SysStatus, status_look, tone_color};
use crate::pages::sys_table::{ActionKindTag, SysDelegate};
use crate::settings_global::{Section, TlSettings};
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    DEMAND_MAX_IDS, HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable,
    TreeTableDelegate,
};
use crate::theme::{TlTheme, current};

/// Static metadata for one Services column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnDef {
    pub id: SysColumnId,
    pub label: &'static str,
    pub default_width: f32,
    pub min_width: f32,
    pub align: ColumnAlign,
    pub visible: bool,
    pub kind: ColumnKind,
    pub template: Option<&'static str>,
    pub heat: bool,
}

const fn service_id(column: ServiceColumn) -> SysColumnId {
    SysColumnId::Services(column)
}

const fn text(
    column: ServiceColumn,
    label: &'static str,
    width: f32,
    min: f32,
    visible: bool,
) -> ColumnDef {
    ColumnDef {
        id: service_id(column),
        label,
        default_width: width,
        min_width: min,
        align: ColumnAlign::Left,
        visible,
        kind: ColumnKind::Text,
        template: None,
        heat: false,
    }
}

const fn number(
    column: ServiceColumn,
    label: &'static str,
    width: f32,
    template: &'static str,
    visible: bool,
    heat: bool,
) -> ColumnDef {
    ColumnDef {
        id: service_id(column),
        label,
        default_width: width,
        min_width: 0.0,
        align: ColumnAlign::Right,
        visible,
        kind: ColumnKind::Numeric,
        template: Some(template),
        heat,
    }
}

static SERVICES_COLUMNS: [ColumnDef; 19] = [
    text(ServiceColumn::Name, "Name", 250.0, 120.0, true),
    text(
        ServiceColumn::DisplayName,
        "Display name",
        300.0,
        60.0,
        true,
    ),
    text(ServiceColumn::Status, "Status", 100.0, 80.0, true),
    text(
        ServiceColumn::StartupType,
        "Startup type",
        120.0,
        80.0,
        true,
    ),
    number(ServiceColumn::Pid, "PID", 80.0, "999999", true, false),
    text(ServiceColumn::User, "User", 200.0, 60.0, true),
    text(ServiceColumn::Path, "Path", 260.0, 60.0, true),
    text(ServiceColumn::Manager, "Manager", 80.0, 60.0, false),
    text(ServiceColumn::UnitType, "Unit type", 90.0, 60.0, false),
    text(ServiceColumn::SubState, "Sub state", 100.0, 60.0, false),
    text(ServiceColumn::LoadState, "Load state", 100.0, 60.0, false),
    number(ServiceColumn::Cpu, "CPU", 63.0, "100.0%", false, true),
    number(
        ServiceColumn::Memory,
        "Memory",
        92.0,
        "999.99 MiB",
        false,
        true,
    ),
    number(ServiceColumn::Disk, "Disk", 82.0, "xxx MiB/s", false, true),
    number(ServiceColumn::Tasks, "Tasks", 72.0, "999999", false, false),
    ColumnDef {
        template: Some("9999-99-99 99:99:99"),
        ..text(
            ServiceColumn::ActiveSince,
            "Active since",
            150.0,
            0.0,
            false,
        )
    },
    text(ServiceColumn::UnitFile, "Unit file", 300.0, 60.0, false),
    text(
        ServiceColumn::TriggeredBy,
        "Triggered by",
        200.0,
        60.0,
        false,
    ),
    text(
        ServiceColumn::Description,
        "Description",
        300.0,
        60.0,
        false,
    ),
];

/// The 19 Services columns in display order.
pub fn services_columns() -> &'static [ColumnDef] {
    &SERVICES_COLUMNS
}

/// True when showing the layout requires unit cgroup samples.
pub fn cgroup_stats_wanted(layout: &ColumnLayout<SysColumnId>) -> bool {
    [
        ServiceColumn::Cpu,
        ServiceColumn::Memory,
        ServiceColumn::Disk,
        ServiceColumn::Tasks,
    ]
    .into_iter()
    .any(|column| layout.visible.contains(&service_id(column)))
}

/// What a child process row shows in one Services column (UI-SYS-016).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChildCell {
    Name,
    Description,
    Pid,
    User,
    ExePath,
    Cpu,
    Memory,
    Disk,
    Threads,
    Empty,
}

/// Services column -> what the child row shows.
pub fn child_column(col: ServiceColumn) -> ChildCell {
    match col {
        ServiceColumn::Name => ChildCell::Name,
        ServiceColumn::DisplayName => ChildCell::Description,
        ServiceColumn::Pid => ChildCell::Pid,
        ServiceColumn::User => ChildCell::User,
        ServiceColumn::Path => ChildCell::ExePath,
        ServiceColumn::Cpu => ChildCell::Cpu,
        ServiceColumn::Memory => ChildCell::Memory,
        ServiceColumn::Disk => ChildCell::Disk,
        ServiceColumn::Tasks => ChildCell::Threads,
        ServiceColumn::Status
        | ServiceColumn::StartupType
        | ServiceColumn::Manager
        | ServiceColumn::UnitType
        | ServiceColumn::SubState
        | ServiceColumn::LoadState
        | ServiceColumn::ActiveSince
        | ServiceColumn::UnitFile
        | ServiceColumn::TriggeredBy
        | ServiceColumn::Description => ChildCell::Empty,
    }
}

/// The process table column a child cell paints, `None` for an empty cell.
fn child_process_column(cell: ChildCell) -> Option<ColumnId> {
    Some(match cell {
        ChildCell::Name => ColumnId::Name,
        ChildCell::Description => ColumnId::Description,
        ChildCell::Pid => ColumnId::Pid,
        ChildCell::User => ColumnId::User,
        ChildCell::ExePath => ColumnId::ImagePath,
        ChildCell::Cpu => ColumnId::Cpu,
        ChildCell::Memory => ColumnId::Memory,
        ChildCell::Disk => ColumnId::Disk,
        ChildCell::Threads => ColumnId::Threads,
        ChildCell::Empty => return None,
    })
}

/// Demand contribution of the Services page: the process sweep and the visible child keys
/// (at most `DEMAND_MAX_IDS`) while at least one unit row is expanded, else nothing.
pub fn child_demand(
    expanded_units: usize,
    visible_child_keys: &[ProcessKey],
) -> (bool, SmallVec<[ProcessKey; 64]>) {
    if expanded_units == 0 {
        return (false, SmallVec::new());
    }
    (
        true,
        visible_child_keys
            .iter()
            .take(DEMAND_MAX_IDS)
            .copied()
            .collect(),
    )
}

/// `sysconf(_SC_CLK_TCK)`, as in the process table.
const CLK_TCK: u32 = 100;

/// Settings sections the child cells read.
const CHILD_SECTIONS: [Section; 3] = [Section::Units, Section::Appearance, Section::Tables];

/// Stable table key for a Services node. `TreeTableDelegate::RowId` must be `Copy`.
pub fn row_key(node: &SysNodeId) -> u64 {
    FxBuildHasher.hash_one(node)
}

fn column_specs() -> Vec<ColumnSpec<SysColumnId>> {
    services_columns()
        .iter()
        .map(|column| {
            let label = SharedString::from(column.label);
            ColumnSpec {
                id: column.id,
                label: label.clone(),
                full_name: label.clone(),
                group: label,
                align: column.align,
                kind: column.kind,
                sortable: true,
                template: column.template,
                default_width: px(column.default_width),
                tooltip: None,
                availability: Availability::Available,
                hideable: column.id != service_id(ServiceColumn::Name),
            }
        })
        .collect()
}

fn saved_sort(state: &UiState) -> SysSortSpec {
    let column = SysColumnId::from_str(
        tl_model::table::SysTable::Services,
        &state.sort.services.column,
    )
    .unwrap_or(service_id(ServiceColumn::Name));
    SysSortSpec {
        column,
        ascending: state.sort.services.ascending,
    }
}

fn manager_index(manager: Manager) -> usize {
    match manager {
        Manager::System => 0,
        Manager::User => 1,
    }
}

/// The Lucide icon of a unit type (UI-SYS-015).
pub fn unit_icon(kind: UnitType) -> IconName {
    match kind {
        UnitType::Service => IconName::Cog,
        UnitType::Socket => IconName::Plug,
        UnitType::Timer => IconName::Clock,
        UnitType::Mount | UnitType::Automount => IconName::HardDrive,
        UnitType::Path => IconName::FolderOpen,
        UnitType::Target => IconName::CircleDot,
        UnitType::Swap => IconName::MemoryStick,
        UnitType::Slice | UnitType::Scope => IconName::Box,
    }
}

/// The label tone of a Startup type word (UI-SYS-015).
pub fn startup_type_tone(t: StartupType) -> LabelTone {
    match t {
        StartupType::Disabled | StartupType::NoUnitFile => LabelTone::Secondary,
        StartupType::Masked => LabelTone::Warning,
        StartupType::Enabled
        | StartupType::EnabledRuntime
        | StartupType::Static
        | StartupType::Indirect
        | StartupType::Generated
        | StartupType::Transient
        | StartupType::Alias
        | StartupType::Linked => LabelTone::Primary,
    }
}

/// The status word of a service status, one to one.
pub fn service_sys_status(s: ServiceStatus) -> SysStatus {
    super::copy::status_word(s)
}

/// The Status label of a unit with `tag` in flight. Actions without an overlay give `None`.
pub fn overlay_label(words: &ServicesWords, tag: ActionKindTag) -> Option<Arc<str>> {
    let ix = match tag {
        ActionKindTag::Start => 0,
        ActionKindTag::Stop => 1,
        ActionKindTag::Restart => 2,
        ActionKindTag::Reload => 3,
        _ => return None,
    };
    Some(words.overlays[ix].clone())
}

/// Writes the label of a manager group row: "System services (214)", the bare name when the
/// manager is unavailable, and " (reconnecting…)" after the count while it reconnects.
pub fn group_label(
    words: &ServicesWords,
    manager: Manager,
    n: u32,
    state: ManagerState,
    out: &mut String,
) {
    let m = manager_index(manager);
    if state == ManagerState::Unavailable {
        out.push_str(&words.group_plain[m]);
        return;
    }
    let _ = words.group_count[m].write(out, &[&CountText(n)]);
    if state == ManagerState::Reconnecting {
        out.push(' ');
        out.push_str(&words.reconnecting);
    }
}

/// Writes the accessible name "<id>, <Status>, <Startup type>".
pub fn unit_accessible_name(words: &ServicesWords, unit: &UnitRecord, out: &mut String) {
    let _ = words.row_name.write(
        out,
        &[
            &&*unit.unit.name,
            &&**words.status(unit.status),
            &&**words.startup(unit.startup),
        ],
    );
}

/// A count written with thin-space groups.
struct CountText(u32);

impl std::fmt::Display for CountText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        units::count(&mut s, u64::from(self.0));
        f.write_str(&s)
    }
}

const SERVICE_STATUSES: [ServiceStatus; 8] = [
    ServiceStatus::Running,
    ServiceStatus::Active,
    ServiceStatus::Starting,
    ServiceStatus::Stopping,
    ServiceStatus::Reloading,
    ServiceStatus::Stopped,
    ServiceStatus::Failed,
    ServiceStatus::Maintenance,
];

const STARTUP_KEYS: [&str; 11] = [
    "services-sys015-enabled",
    "services-sys015-enabled-runtime",
    "services-sys015-disabled",
    "services-sys015-static",
    "services-sys015-indirect",
    "services-sys015-masked",
    "services-sys015-generated",
    "services-sys015-transient",
    "services-sys015-alias",
    "services-sys015-linked",
    "services-sys015-no-unit-file",
];

const TYPE_KEYS: [&str; 10] = [
    "services-sys015-service",
    "services-sys015-socket",
    "services-sys015-timer",
    "services-sys015-mount",
    "services-sys015-automount",
    "services-sys015-path",
    "services-sys015-target",
    "services-sys015-swap",
    "services-sys015-slice",
    "services-sys015-scope",
];

fn startup_index(t: StartupType) -> usize {
    match t {
        StartupType::Enabled => 0,
        StartupType::EnabledRuntime => 1,
        StartupType::Disabled => 2,
        StartupType::Static => 3,
        StartupType::Indirect => 4,
        StartupType::Masked => 5,
        StartupType::Generated => 6,
        StartupType::Transient => 7,
        StartupType::Alias => 8,
        StartupType::Linked => 9,
        StartupType::NoUnitFile => 10,
    }
}

fn type_index(t: UnitType) -> usize {
    match t {
        UnitType::Service => 0,
        UnitType::Socket => 1,
        UnitType::Timer => 2,
        UnitType::Mount => 3,
        UnitType::Automount => 4,
        UnitType::Path => 5,
        UnitType::Target => 6,
        UnitType::Swap => 7,
        UnitType::Slice => 8,
        UnitType::Scope => 9,
    }
}

/// The raw systemd ActiveState word of a status, for the Status tooltip.
fn active_word(s: ServiceStatus) -> &'static str {
    match s {
        ServiceStatus::Running | ServiceStatus::Active => "active",
        ServiceStatus::Starting => "activating",
        ServiceStatus::Stopping => "deactivating",
        ServiceStatus::Reloading => "reloading",
        ServiceStatus::Stopped => "inactive",
        ServiceStatus::Failed => "failed",
        ServiceStatus::Maintenance => "maintenance",
    }
}

/// The Services words, translated once per strings version (R-132).
#[derive(Clone, Debug)]
pub struct ServicesWords {
    /// The strings version, `None` without strings.
    version: Option<u64>,
    /// Status labels in [`SERVICE_STATUSES`] order.
    statuses: [Arc<str>; 8],
    startups: [Arc<str>; 11],
    types: [Arc<str>; 10],
    /// System, User.
    managers: [Arc<str>; 2],
    /// Starting, Stopping, Restarting, Reloading.
    overlays: [Arc<str>; 4],
    /// "System services ({ $n })" and the user form.
    group_count: [Template; 2],
    /// "System services" and the user form.
    group_plain: [SharedString; 2],
    not_available: [SharedString; 2],
    reconnecting: SharedString,
    group_name: Template,
    expanded: SharedString,
    collapsed: SharedString,
    manager_line: [SharedString; 2],
    row_name: Template,
}

impl ServicesWords {
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
            statuses: SERVICE_STATUSES.map(|s| arc(service_sys_status(s).key())),
            startups: STARTUP_KEYS.map(arc),
            types: TYPE_KEYS.map(arc),
            managers: ["services-sys015-system", "services-sys015-user"].map(arc),
            overlays: [
                "services-sys018-starting",
                "services-sys018-stopping",
                "services-sys018-restarting",
                "services-sys018-reloading",
            ]
            .map(arc),
            group_count: [
                template("services-sys014-system-services-n", &["n"]),
                template("services-sys014-user-services-n", &["n"]),
            ],
            group_plain: [
                text("services-sys014-system-services"),
                text("services-sys014-user-services"),
            ],
            not_available: [
                text("services-sys014-system-manager-not-available"),
                text("services-sys014-user-manager-not-available"),
            ],
            reconnecting: text("services-sys014-reconnecting"),
            group_name: template("services-sys014-group-name", &["group", "n", "state"]),
            expanded: text("services-sys014-expanded"),
            collapsed: text("services-sys014-collapsed"),
            manager_line: [
                text("services-sys015-manager-system"),
                text("services-sys015-manager-user"),
            ],
            row_name: template("services-sys015-row-name", &["id", "status", "startup"]),
        }
    }

    fn status(&self, s: ServiceStatus) -> &Arc<str> {
        &self.statuses[s as usize]
    }

    fn startup(&self, t: StartupType) -> &Arc<str> {
        &self.startups[startup_index(t)]
    }
}

/// The hovered row's tooltip lines, shared between the delegate and [`ServicesRowTip`].
#[derive(Default)]
struct RowTipState {
    /// (table version, row index) the lines were built for.
    key: Option<(u64, usize)>,
    lines: Vec<SharedString>,
}

/// The Name cell tooltip view: description, unit file path and manager.
pub struct ServicesRowTip {
    state: Rc<RefCell<RowTipState>>,
    built: Option<((u64, usize), Entity<TlTooltip>)>,
}

impl Render for ServicesRowTip {
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

/// Fills a metric cell from a `Total`: the bits, the validity and, when asked, the text.
fn metric<T: Copy>(
    out: &mut CellOut,
    total: Total<T>,
    bits: impl Fn(T) -> u64,
    write: impl Fn(&mut String, T),
) {
    out.validity = total.validity;
    out.bits = bits(total.value);
    if out.want_text && matches!(total.validity, Validity::Valid | Validity::Stale) {
        write(&mut out.text, total.value);
    }
}

/// Services rows and table-owned interaction state.
pub struct ServicesDelegate {
    snap: Arc<ServicesSnapshot>,
    /// The whole installed snapshot. Child rows read their process records here.
    snapshot: Arc<Snapshot>,
    rows: Arc<[SysRow]>,
    row_index: FxHashMap<u64, usize>,
    expanded: FxHashSet<SysNodeId>,
    specs: Vec<ColumnSpec<SysColumnId>>,
    layout: ColumnLayout<SysColumnId>,
    sort: SysSortSpec,
    backend: ServiceBackend,
    sort_request: Option<SysSortSpec>,
    expanded_request: Option<Arc<[SysNodeId]>>,
    version: u64,
    /// Row installs: one per services view shown.
    row_builds: u64,
    active: bool,
    window: Option<AnyWindowHandle>,
    words: ServicesWords,
    /// `None` until the first `refresh_look`: cells then set no colours.
    theme: Option<Arc<TlTheme>>,
    /// Units with an action in flight, for the Status overlay (UI-SYS-018).
    pending: FxHashMap<UnitRef, ActionKindTag>,
    /// The Name text of the two group rows, built once per snapshot.
    group_labels: [SharedString; 2],
    tip: Rc<RefCell<RowTipState>>,
    tip_view: Option<AnyView>,
    /// Units, appearance and tables settings for the child cells.
    settings: Settings,
    /// The `CHILD_SECTIONS` versions `settings` was read at.
    settings_seen: Option<[u64; 3]>,
    cell_words: CellWords,
    name_words: NameWords,
    tips: ValidityTips,
    /// Never refreshed: child rows are records, which do not read it.
    members: GroupMembers,
    /// The child process keys of the last visible range, at most `DEMAND_MAX_IDS`.
    visible_child_keys: Arc<[ProcessKey]>,
    /// The hovered child row's tooltip lines, shared with `proc_tip_view`.
    proc_tip: Rc<RefCell<ProcTipState>>,
    proc_tip_view: Option<AnyView>,
    proc_tip_words: RowTipWords,
    /// The page that builds the row menu and keeps its captured targets.
    page: Option<WeakEntity<super::ServicesPage>>,
}

impl ServicesDelegate {
    pub fn new(state: &UiState) -> Self {
        let specs = column_specs();
        let mut expanded = FxHashSet::default();
        if state.services.group_expanded.system {
            expanded.insert(SysNodeId::ServiceGroup(Manager::System));
        }
        if state.services.group_expanded.user {
            expanded.insert(SysNodeId::ServiceGroup(Manager::User));
        }
        Self {
            snap: Arc::default(),
            snapshot: Arc::new(Snapshot::empty(BootId(0), 0)),
            rows: Arc::from([]),
            row_index: FxHashMap::default(),
            expanded,
            layout: ColumnLayout::from_state(TableId::Services, &state.columns.services, &specs),
            specs,
            sort: saved_sort(state),
            backend: ServiceBackend::Systemd,
            sort_request: None,
            expanded_request: None,
            version: 0,
            row_builds: 0,
            active: false,
            window: None,
            words: ServicesWords::en(),
            theme: None,
            pending: FxHashMap::default(),
            group_labels: [SharedString::default(), SharedString::default()],
            tip: Rc::default(),
            tip_view: None,
            settings: Settings::default(),
            settings_seen: None,
            cell_words: CellWords::from_fn(|key| Arc::from(tr_en(key, &[]).as_ref())),
            name_words: NameWords::from_fn(|key| tr_en(key, &[])),
            tips: ValidityTips::default(),
            members: GroupMembers::default(),
            visible_child_keys: Arc::from([]),
            proc_tip: Rc::default(),
            proc_tip_view: None,
            proc_tip_words: RowTipWords::from_fn(|key| tr_en(key, &[])),
            page: None,
        }
    }

    pub(super) fn set_page(&mut self, page: WeakEntity<super::ServicesPage>) {
        self.page = Some(page);
    }

    /// Ctrl+L: collapses every unit row and expands both group rows (UI-SYS-014). The groups'
    /// state is saved as on a toggle.
    pub fn collapse_units(&mut self, cx: &mut Context<TlTreeTable<Self>>) {
        let groups = [
            SysNodeId::ServiceGroup(Manager::System),
            SysNodeId::ServiceGroup(Manager::User),
        ];
        if self.expanded.len() == groups.len() && groups.iter().all(|g| self.expanded.contains(g)) {
            return;
        }
        self.expanded.clear();
        self.expanded.extend(groups);
        if cx.has_global::<TlSettings>() {
            TlSettings::update(cx, |data| {
                data.state.services.group_expanded.system = true;
                data.state.services.group_expanded.user = true;
            });
        }
        self.expanded_request = Some(self.requested_expanded());
        self.send_demand(cx);
        cx.notify();
    }

    /// Expands `manager`'s group for a jump to one of its units and saves the group's state as
    /// a toggle does. Returns the expanded set for the page's query. It sets no
    /// `expanded_request`: the caller sends the query itself.
    pub fn expand_group(
        &mut self,
        manager: Manager,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> Arc<[SysNodeId]> {
        if self.expanded.insert(SysNodeId::ServiceGroup(manager)) && cx.has_global::<TlSettings>() {
            TlSettings::update(cx, |data| match manager {
                Manager::System => data.state.services.group_expanded.system = true,
                Manager::User => data.state.services.group_expanded.user = true,
            });
        }
        cx.notify();
        self.requested_expanded()
    }

    /// Marks `unit` as having `tag` in flight, or clears the mark with `None`.
    pub fn set_pending(&mut self, unit: UnitRef, tag: Option<ActionKindTag>) {
        let changed = match tag {
            Some(tag) => self.pending.insert(unit, tag) != Some(tag),
            None => self.pending.remove(&unit).is_some(),
        };
        if changed {
            self.version += 1;
        }
    }

    /// Re-reads the words and theme when they changed. Returns true on a change.
    pub fn refresh_look(&mut self, cx: &App) -> bool {
        let mut changed = false;
        let version = cx.try_global::<Strings>().map(|s| s.strings_version);
        if version != self.words.version {
            self.words = ServicesWords::new(cx);
            self.build_group_labels();
            self.cell_words = CellWords::new(cx);
            self.name_words = NameWords::new(cx);
            self.proc_tip_words = RowTipWords::new(cx);
            self.proc_tip.borrow_mut().clear();
            changed = true;
        }
        if let Some(settings) = cx.try_global::<TlSettings>() {
            let seen = CHILD_SECTIONS.map(|section| settings.version(section));
            if self.settings_seen != Some(seen) {
                self.settings_seen = Some(seen);
                self.settings = TlSettings::settings(cx).clone();
                changed = true;
            }
        }
        let theme = current(cx);
        if self.theme.as_ref().is_none_or(|t| !Arc::ptr_eq(t, theme)) {
            self.theme = Some(theme.clone());
            changed = true;
        }
        if changed {
            self.version += 1;
        }
        changed
    }

    fn build_group_labels(&mut self) {
        for manager in [Manager::System, Manager::User] {
            let m = manager_index(manager);
            let state = match self.snap.managers[m] {
                ManagerState::Reconnecting => ManagerState::Connected,
                other => other,
            };
            let mut text = String::new();
            group_label(
                &self.words,
                manager,
                self.snap.manager_counts[m],
                state,
                &mut text,
            );
            self.group_labels[m] = text.into();
        }
    }

    fn unit(&self, ix: usize) -> Option<&UnitRecord> {
        let row = self.rows.get(ix)?;
        if !matches!(row.node, SysNodeId::Unit(_)) {
            return None;
        }
        self.snap.units.get(row.record? as usize)
    }

    /// The process key of child row `ix` and the index of its record in the installed process
    /// snapshot, `None` when the process left it. Services rows rebuild only on services
    /// updates, so `SysRow.record` can point into an older process snapshot and is not read.
    fn child(&self, ix: usize) -> Option<(ProcessKey, Option<usize>)> {
        let SysNodeId::UnitProcess(_, key) = self.rows.get(ix)?.node else {
            return None;
        };
        let records = &self.snapshot.processes.records;
        Some((key, records.binary_search_by_key(&key, |r| r.key).ok()))
    }

    fn child_record(&self, i: usize) -> (&ProcessRecord, TableRow) {
        let r = &self.snapshot.processes.records[i];
        let row = TableRow {
            node: NodeId::Process(r.key),
            depth: 2,
            expanded: false,
            has_children: false,
            record: Some(i as u32),
            group: None,
            descendants: 0,
            subtree: None,
        };
        (r, row)
    }

    fn cell_ctx(&self) -> CellCtx<'_> {
        let s = &*self.snapshot;
        let system = &s.system;
        CellCtx {
            view: &s.table,
            records: &s.processes.records,
            extras: &s.processes.extras,
            extras_p2: &s.processes.extras_p2,
            settings: &self.settings,
            words: &self.cell_words,
            members: &self.members,
            ncpu: system.cpu_topology.online.count().min(u32::from(u16::MAX)) as u16,
            online: &system.cpu_topology.online,
            mem_total: system.memory.total,
            vram_total: system.gpus.iter().map(|(_, g)| g.mem_total.value).sum(),
            boot_time_s: system.boot_time_s,
            clk_tck: CLK_TCK,
        }
    }

    /// One cell of child row `ix` through the process table's cells (E22). A process that left
    /// the snapshot shows only its PID, in `text_secondary`, until the row goes.
    fn child_cell(
        &self,
        key: ProcessKey,
        record: Option<usize>,
        column: ServiceColumn,
        out: &mut CellOut,
    ) {
        out.validity = Validity::Valid;
        let (Some(col), Some(theme)) = (child_process_column(child_column(column)), &self.theme)
        else {
            return;
        };
        let Some(i) = record else {
            if col == ColumnId::Pid {
                out.bits = u64::from(key.pid);
                if out.want_text {
                    let _ = write!(out.text, "{}", key.pid);
                }
            }
            out.text_color = Some(theme.text_secondary);
            return;
        };
        let (r, row) = self.child_record(i);
        let exited = r.flags.contains(ProcFlags::EXITED);
        let a = &self.settings.appearance;
        let state = PaintState {
            selected: false,
            exited,
            heat_on: a.heat_coloring,
            heat_intensity_pct: a.heat_intensity_pct,
            dim_zero: self.settings.tables.dim_zero_values,
            subtree: false,
        };
        ProcessRowCells::fill(
            RowRef::Record(r, &row, RowKind::Process),
            col,
            &self.cell_ctx(),
            state,
            theme,
            &self.tips,
            out,
        );
        if out.want_tooltip && matches!(out.validity, Validity::Stale | Validity::Failed) {
            out.tooltip = live_tip(out.validity, &self.snapshot, now_boot_ns());
        }
        if exited {
            // UI-PRC-011: all text of an exited row is secondary.
            out.text_color = Some(theme.text_secondary);
        }
    }

    /// Unit rows expanded now.
    fn expanded_units(&self) -> usize {
        self.expanded
            .iter()
            .filter(|node| matches!(node, SysNodeId::Unit(_)))
            .count()
    }

    /// Units of a reconnecting manager draw every cell's text in `text_secondary`, including
    /// toned labels and the pending overlay.
    fn mute_when_reconnecting(&self, unit: &UnitRecord, out: &mut CellOut) {
        if let (Some(theme), ManagerState::Reconnecting) = (
            &self.theme,
            self.snap.managers[manager_index(unit.unit.manager)],
        ) {
            out.text_color = Some(theme.text_secondary);
            out.selected_text_color = Some(theme.text_selected_secondary);
        }
    }

    fn status_cell(&self, unit: &UnitRecord, out: &mut CellOut) {
        let look = status_look(service_sys_status(unit.status));
        let overlay = self
            .pending
            .get(&unit.unit)
            .and_then(|tag| overlay_label(&self.words, *tag));
        if let Some(label) = overlay {
            out.arc = Some(label);
            out.dot = Some(DotStyle::Filled(LabelTone::Info));
        } else {
            out.arc = Some(self.words.status(unit.status).clone());
            out.dot = Some(look.dot);
            if let (Some(theme), true) = (&self.theme, look.label != LabelTone::Primary) {
                out.text_color = Some(tone_color(look.label, theme, false));
                out.selected_text_color = Some(tone_color(look.label, theme, true));
            }
        }
        if out.want_tooltip {
            out.tooltip = Some(format!("{} ({})", active_word(unit.status), unit.sub_state).into());
        }
    }

    /// The accessible row name: "<id>, <Status>, <Startup type>" for a unit and
    /// "System services, 214 units, expanded" for a group.
    pub fn accessible_name(&self, ix: usize) -> Option<String> {
        let mut out = String::new();
        if let Some(unit) = self.unit(ix) {
            unit_accessible_name(&self.words, unit, &mut out);
            return Some(out);
        }
        let SysNodeId::ServiceGroup(manager) = self.rows.get(ix)?.node else {
            return None;
        };
        let m = manager_index(manager);
        let words = &self.words;
        if self.snap.managers[m] == ManagerState::Unavailable {
            out.push_str(&words.group_plain[m]);
            out.push_str(", ");
            out.push_str(&words.not_available[m]);
            return Some(out);
        }
        let state = if self.expanded.contains(&SysNodeId::ServiceGroup(manager)) {
            &words.expanded
        } else {
            &words.collapsed
        };
        let _ = words.group_name.write(
            &mut out,
            &[
                &&*words.group_plain[m],
                &CountText(self.snap.manager_counts[m]),
                &&**state,
            ],
        );
        Some(out)
    }

    pub fn default_layout(&self) -> ColumnLayout<SysColumnId> {
        let order: Vec<_> = services_columns().iter().map(|column| column.id).collect();
        let visible: Vec<_> = services_columns()
            .iter()
            .filter(|column| column.visible)
            .map(|column| column.id)
            .collect();
        ColumnLayout::defaults(TableId::Services, &self.specs, &order, &visible)
    }

    pub fn set_window(&mut self, window: Option<AnyWindowHandle>) {
        self.window = window;
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<TlTreeTable<Self>>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.send_demand(cx);
        } else if let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) {
            let source = cx.entity_id();
            demand.update(cx, |demand, cx| demand.remove(source, cx));
        }
    }

    /// The services list, unit cgroup stats for the shown metric columns and, while a unit is
    /// expanded, the process sweep with the visible child keys (UI-SYS-016).
    pub fn demand_part(&self) -> Demand {
        let (process_table, keys) = child_demand(self.expanded_units(), &self.visible_child_keys);
        let mut process_columns = ColumnSet::EMPTY;
        if process_table {
            for col in self.layout.visible_columns().iter() {
                if let SysColumnId::Services(column) = col
                    && let Some(id) = child_process_column(child_column(*column))
                {
                    process_columns.insert(id);
                }
            }
        }
        Demand {
            any_surface_visible: true,
            services: true,
            service_cgroup_stats: cgroup_stats_wanted(&self.layout),
            process_table,
            process_columns,
            detail_keys: keys.as_slice().into(),
            ..Demand::default()
        }
    }

    fn send_demand(&self, cx: &mut Context<TlTreeTable<Self>>) {
        if !self.active {
            return;
        }
        let Some(demand) = cx.try_global::<GlobalDemand>().map(|d| d.0.clone()) else {
            return;
        };
        let (source, window, part) = (cx.entity_id(), self.window, self.demand_part());
        demand.update(cx, |demand, cx| demand.set(source, window, part, cx));
    }

    pub fn take_sort_request(&mut self) -> Option<SysSortSpec> {
        self.sort_request.take()
    }

    /// A process update with no services update. With a unit row expanded, the child rows read
    /// the new process records; the rows, their index and the group labels stay. False, and
    /// nothing changes, while every unit row is collapsed.
    pub fn install_processes(&mut self, snap: &Arc<Snapshot>) -> bool {
        if self.expanded_units() == 0 {
            return false;
        }
        self.snapshot = Arc::clone(snap);
        self.version += 1;
        true
    }

    /// How many times the rows were installed, for the re-render tests.
    pub fn row_builds(&self) -> u64 {
        self.row_builds
    }

    pub fn take_expanded_request(&mut self) -> Option<Arc<[SysNodeId]>> {
        self.expanded_request.take()
    }

    pub fn expanded(&self) -> &FxHashSet<SysNodeId> {
        &self.expanded
    }

    pub fn backend(&self) -> ServiceBackend {
        self.backend
    }

    pub fn services(&self) -> &Arc<ServicesSnapshot> {
        &self.snap
    }

    pub fn processes(&self) -> &Arc<ProcessSnapshot> {
        &self.snapshot.processes
    }

    fn requested_expanded(&self) -> Arc<[SysNodeId]> {
        self.expanded.iter().cloned().collect()
    }
}

impl TreeTableDelegate for ServicesDelegate {
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
        self.row_index.get(&id).copied()
    }
    fn row_kind(&self, ix: usize) -> RowKind {
        match self.rows[ix].node {
            SysNodeId::ServiceGroup(_) => RowKind::GroupHeader,
            SysNodeId::Unit(_) => RowKind::Unit,
            SysNodeId::UnitProcess(_, _) => RowKind::Process,
            _ => RowKind::Plain,
        }
    }
    fn row_depth(&self, ix: usize) -> u8 {
        self.rows[ix].depth
    }
    fn row_expandable(&self, ix: usize) -> Option<bool> {
        let row = self.rows.get(ix)?;
        match &row.node {
            SysNodeId::ServiceGroup(manager)
                if self.snap.managers[manager_index(*manager)] == ManagerState::Unavailable =>
            {
                None
            }
            SysNodeId::ServiceGroup(_) => Some(self.expanded.contains(&row.node)),
            SysNodeId::Unit(_) if row.has_children => Some(self.expanded.contains(&row.node)),
            _ => None,
        }
    }
    fn row_flags(&self, ix: usize) -> RowFlags {
        if let Some((_, record)) = self.child(ix) {
            let Some(i) = record else {
                return RowFlags {
                    bits: RowFlags::EXITED,
                    ..RowFlags::default()
                };
            };
            let r = &self.snapshot.processes.records[i];
            let mut bits = 0;
            if r.flags.contains(ProcFlags::NEW) {
                bits |= RowFlags::NEW;
            }
            if r.flags.contains(ProcFlags::EXITED) {
                bits |= RowFlags::EXITED;
            }
            if r.state == ProcState::Stopped {
                bits |= RowFlags::SUSPENDED;
            }
            if r.validity.0.contains(&Validity::Restricted) {
                bits |= RowFlags::RESTRICTED;
            }
            return RowFlags {
                bits,
                ..RowFlags::default()
            };
        }
        match self.unit(ix) {
            Some(unit)
                if self.snap.managers[manager_index(unit.unit.manager)]
                    == ManagerState::Reconnecting =>
            {
                RowFlags {
                    bits: RowFlags::STALE | RowFlags::SECONDARY_NAME,
                    ..RowFlags::default()
                }
            }
            _ => RowFlags::default(),
        }
    }
    fn table_version(&self) -> u64 {
        self.version
    }
    fn columns(&self) -> &ColumnLayout<SysColumnId> {
        &self.layout
    }
    fn column_spec(&self, id: SysColumnId) -> &ColumnSpec<SysColumnId> {
        self.specs
            .iter()
            .find(|spec| spec.id == id)
            .unwrap_or(&self.specs[0])
    }
    fn cell(&self, ix: usize, col: SysColumnId, out: &mut CellOut) {
        if let (Some((key, record)), SysColumnId::Services(column)) = (self.child(ix), col) {
            self.child_cell(key, record, column, out);
            return;
        }
        let (Some(unit), SysColumnId::Services(column)) = (self.unit(ix), col) else {
            return;
        };
        let w = &self.words;
        out.validity = Validity::Valid;
        match column {
            ServiceColumn::Name => out.arc = Some(unit.unit.name.clone()),
            ServiceColumn::DisplayName | ServiceColumn::Description => {
                out.arc = Some(unit.description.clone());
            }
            ServiceColumn::Status => self.status_cell(unit, out),
            ServiceColumn::StartupType => {
                out.arc = Some(w.startup(unit.startup).clone());
                let tone = startup_type_tone(unit.startup);
                if let (Some(theme), true) = (&self.theme, tone != LabelTone::Primary) {
                    out.text_color = Some(tone_color(tone, theme, false));
                    out.selected_text_color = Some(tone_color(tone, theme, true));
                }
            }
            ServiceColumn::Pid => {
                out.bits = u64::from(unit.main_pid);
                if out.want_text && unit.main_pid != 0 {
                    units::count(&mut out.text, u64::from(unit.main_pid));
                }
            }
            ServiceColumn::User => out.arc = Some(unit.user.clone()),
            ServiceColumn::Path => out.arc = Some(unit.path.clone()),
            ServiceColumn::Manager => {
                out.arc = Some(w.managers[manager_index(unit.unit.manager)].clone())
            }
            ServiceColumn::UnitType => out.arc = Some(w.types[type_index(unit.kind)].clone()),
            ServiceColumn::SubState => out.arc = Some(unit.sub_state.clone()),
            ServiceColumn::LoadState => out.arc = Some(unit.load_state.clone()),
            ServiceColumn::Cpu => metric(
                out,
                unit.cpu_pct,
                |v| u64::from(v.to_bits()),
                units::percent,
            ),
            ServiceColumn::Memory => metric(out, unit.memory_bytes, |v| v, units::bytes),
            ServiceColumn::Disk => metric(out, unit.disk_bps, f64::to_bits, units::rate_mib_fixed),
            ServiceColumn::Tasks => metric(out, unit.tasks, |v| v, units::count),
            ServiceColumn::ActiveSince => {
                if let Some(us) = unit.active_since_us {
                    let secs = us / 1_000_000;
                    out.bits = secs;
                    if out.want_text {
                        local_time(secs, &mut out.text);
                    }
                }
            }
            ServiceColumn::UnitFile => out.arc = unit.unit_file.clone(),
            ServiceColumn::TriggeredBy => {
                out.bits = FxBuildHasher.hash_one(&*unit.triggered_by);
                if out.want_text {
                    for (i, id) in unit.triggered_by.iter().enumerate() {
                        if i > 0 {
                            out.text.push_str(", ");
                        }
                        out.text.push_str(id);
                    }
                }
            }
        }
        self.mute_when_reconnecting(unit, out);
    }
    fn name_cell(&self, ix: usize) -> NameCell {
        let mut cell = NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::default(),
            count_run: None,
            secondary: None,
            glyphs: SmallVec::new(),
            highlight_fffd: false,
            trailing: None,
        };
        if let Some((_, record)) = self.child(ix) {
            let mut d = NameCellData {
                kind: RowKind::Process,
                depth: 2,
                expandable: None,
                icon: NameIcon::None,
                primary: SharedString::default(),
                count_run: None,
                state_glyph: None,
                exited: true,
                secondary: None,
            };
            if let Some(i) = record {
                let r = &self.snapshot.processes.records[i];
                d.icon = r
                    .meta
                    .icon
                    .as_ref()
                    .map_or(NameIcon::None, |i| NameIcon::File(Arc::clone(i)));
                d.primary = SharedString::from(&r.meta.name);
                d.exited = r.flags.contains(ProcFlags::EXITED);
                if r.state == ProcState::Stopped {
                    d.state_glyph = Some(StateGlyph::Suspended);
                }
            }
            return name_cell::name_cell(d, &self.name_words);
        }
        if let Some(unit) = self.unit(ix) {
            cell.icon = NameIcon::Lucide(unit_icon(unit.kind));
            cell.primary = SharedString::from(unit.unit.name.clone());
        } else if let Some(SysNodeId::ServiceGroup(manager)) = self.rows.get(ix).map(|r| &r.node) {
            let m = manager_index(*manager);
            cell.primary = self.group_labels[m].clone();
            match self.snap.managers[m] {
                ManagerState::Unavailable => {
                    cell.glyphs
                        .push((IconName::CircleAlert, SharedString::default()));
                    cell.secondary = Some(self.words.not_available[m].clone());
                }
                ManagerState::Reconnecting => {
                    cell.secondary = Some(self.words.reconnecting.clone())
                }
                ManagerState::Connected => {}
            }
        }
        cell
    }
    fn name_column(&self) -> Option<SysColumnId> {
        Some(service_id(ServiceColumn::Name))
    }
    fn header_total(&self, _: SysColumnId, _: &mut CellOut) -> bool {
        false
    }
    fn sort(&self) -> SysSortSpec {
        self.sort
    }
    fn selectable(&self, ix: usize) -> bool {
        matches!(
            self.rows[ix].node,
            SysNodeId::Unit(_) | SysNodeId::UnitProcess(_, _)
        )
    }
    fn context_menu(
        &mut self,
        targets: &[u64],
        clicked_col: Option<SysColumnId>,
        menu: TlMenu,
        window: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        let Some(page) = self.page.clone() else {
            return menu;
        };
        let nodes: SmallVec<[SysNodeId; 8]> = targets
            .iter()
            .filter_map(|id| Some(self.rows.get(self.row_index(*id)?)?.node.clone()))
            .collect();
        super::row_menu(&page, self, &nodes, clicked_col, menu, window, cx)
    }
    fn on_menu_closed(&mut self, cx: &mut Context<TlTreeTable<Self>>) {
        if let Some(page) = self.page.as_ref().and_then(WeakEntity::upgrade) {
            page.update(cx, |page, _| page.clear_menu_targets());
        }
    }
    fn on_toggle_expand(&mut self, id: u64, cx: &mut Context<TlTreeTable<Self>>) {
        let Some(node) = self
            .row_index(id)
            .and_then(|ix| self.rows.get(ix))
            .map(|row| row.node.clone())
        else {
            return;
        };
        if !self.expanded.remove(&node) {
            self.expanded.insert(node.clone());
        }
        if let SysNodeId::ServiceGroup(manager) = node
            && cx.has_global::<TlSettings>()
        {
            let expanded = self.expanded.contains(&SysNodeId::ServiceGroup(manager));
            TlSettings::update(cx, |data| match manager {
                Manager::System => data.state.services.group_expanded.system = expanded,
                Manager::User => data.state.services.group_expanded.user = expanded,
            });
        }
        self.expanded_request = Some(self.requested_expanded());
        self.send_demand(cx);
        cx.notify();
    }
    fn on_sort(&mut self, column: SysColumnId, cx: &mut Context<TlTreeTable<Self>>) {
        let ascending = if self.sort.column == column {
            !self.sort.ascending
        } else {
            true
        };
        self.sort = SysSortSpec { column, ascending };
        self.sort_request = Some(self.sort);
        if cx.has_global::<TlSettings>() {
            let saved = SortState {
                column: column.as_str().to_owned(),
                ascending,
            };
            TlSettings::update(cx, |data| data.state.sort.services = saved);
        }
        cx.notify();
    }
    fn on_columns_changed(
        &mut self,
        layout: &ColumnLayout<SysColumnId>,
        cx: &mut Context<TlTreeTable<Self>>,
    ) {
        self.layout = layout.clone();
        if cx.has_global::<TlSettings>() {
            let saved = layout.to_state();
            TlSettings::update(cx, |data| data.state.columns.services = saved);
        }
        self.send_demand(cx);
    }
    fn on_visible_range(&mut self, ids: &[u64], cx: &mut Context<TlTreeTable<Self>>) {
        let keys: Arc<[ProcessKey]> = ids
            .iter()
            .filter_map(|id| match self.rows.get(self.row_index(*id)?)?.node {
                SysNodeId::UnitProcess(_, key) => Some(key),
                _ => None,
            })
            .take(DEMAND_MAX_IDS)
            .collect();
        if keys != self.visible_child_keys {
            self.visible_child_keys = keys;
            self.send_demand(cx);
        }
    }
    fn on_activate(&mut self, _: u64, cx: &mut Context<TlTreeTable<Self>>) {
        cx.propagate();
    }
    fn row_tooltip(&self, ix: usize) -> Option<AnyView> {
        if let Some((_, record)) = self.child(ix) {
            let view = self.proc_tip_view.as_ref()?;
            let (r, row) = self.child_record(record?);
            let key = (self.snapshot.sequence, self.version, ix);
            return self
                .proc_tip
                .borrow_mut()
                .update(
                    key,
                    RowRef::Record(r, &row, RowKind::Process),
                    &self.snapshot,
                    &self.proc_tip_words,
                )
                .then(|| view.clone());
        }
        let view = self.tip_view.as_ref()?;
        let unit = self.unit(ix)?;
        let key = (self.version, ix);
        let mut tip = self.tip.borrow_mut();
        if tip.key != Some(key) {
            tip.key = Some(key);
            tip.lines.clear();
            tip.lines.push(unit.description.clone().into());
            if let Some(path) = &unit.unit_file {
                tip.lines.push(path.clone().into());
            }
            tip.lines
                .push(self.words.manager_line[manager_index(unit.unit.manager)].clone());
            tip.lines.retain(|line| !line.is_empty());
        }
        Some(view.clone())
    }
    fn cell_tip_delay(&self) -> Duration {
        DELAY_ROW_NAME
    }
    fn before_rows(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<TlTreeTable<Self>>,
    ) {
        if self.tip_view.is_none() {
            let state = Rc::clone(&self.tip);
            self.tip_view = Some(cx.new(|_| ServicesRowTip { state, built: None }).into());
        }
        if self.proc_tip_view.is_none() {
            let state = Rc::clone(&self.proc_tip);
            self.proc_tip_view = Some(cx.new(|_| RowTooltip::new(state)).into());
        }
        if self.refresh_look(cx) {
            cx.notify();
        }
    }
    fn empty_text(&self) -> Option<SharedString> {
        None
    }
    fn header_lines(&self) -> HeaderLines {
        HeaderLines::One
    }
    fn columns_fixed(&self) -> bool {
        false
    }
}

impl SysDelegate for ServicesDelegate {
    fn component_header<'a>(&self, snap: &'a Snapshot) -> &'a ComponentHeader {
        &snap.services.header
    }
    fn rows_len(&self, snap: &Snapshot) -> usize {
        snap.services.rows.len()
    }
    fn query_generation(&self, snap: &Snapshot) -> u64 {
        snap.services.query_generation
    }
    fn install(&mut self, snap: &Arc<Snapshot>) {
        self.snap = Arc::clone(&snap.services);
        self.snapshot = Arc::clone(snap);
        self.rows = Arc::clone(&self.snap.rows);
        self.row_index = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| (row_key(&row.node), ix))
            .collect();
        self.backend = self.snap.backend;
        self.build_group_labels();
        self.version += 1;
        self.row_builds += 1;
    }
    fn mask(&self) -> ComponentMask {
        ComponentMask::SERVICES | ComponentMask::PROCESSES
    }
}
