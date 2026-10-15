//! Services table rows, columns, expansion and persistence (UI-SYS-002, UI-SYS-013).

use std::hash::BuildHasher as _;
use std::sync::Arc;

use gpui_kit::{AnyView, AnyWindowHandle, Context, SharedString, Window, px};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use tl_model::demand::Demand;
use tl_model::health::ManagerState;
use tl_model::settings::{SortState, UiState};
use tl_model::snapshot::{
    ComponentHeader, ComponentMask, ProcessSnapshot, ServiceBackend, ServicesSnapshot, Snapshot,
};
use tl_model::table::{Manager, ServiceColumn, SysColumnId, SysNodeId, SysRow, SysSortSpec};

use crate::components::menu::TlMenu;
use crate::demand::GlobalDemand;
use crate::pages::sys_table::SysDelegate;
use crate::settings_global::TlSettings;
use crate::table::cells::CellOut;
use crate::table::columns::{
    Availability, ColumnAlign, ColumnKind, ColumnLayout, ColumnSpec, TableId,
};
use crate::table::tree_table::{
    HeaderLines, IconSlot, NameCell, NameIcon, RowFlags, RowKind, TlTreeTable, TreeTableDelegate,
};

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

/// Services rows and table-owned interaction state.
pub struct ServicesDelegate {
    snap: Arc<ServicesSnapshot>,
    processes: Arc<ProcessSnapshot>,
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
    active: bool,
    window: Option<AnyWindowHandle>,
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
            processes: Arc::default(),
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
            active: false,
            window: None,
        }
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

    pub fn demand_part(&self) -> Demand {
        Demand {
            any_surface_visible: true,
            services: true,
            service_cgroup_stats: cgroup_stats_wanted(&self.layout),
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
        &self.processes
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
    fn row_flags(&self, _: usize) -> RowFlags {
        RowFlags::default()
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
    fn cell(&self, _: usize, _: SysColumnId, _: &mut CellOut) {}
    fn name_cell(&self, _: usize) -> NameCell {
        NameCell {
            icon: NameIcon::None,
            icon_size: IconSlot::Small,
            primary: SharedString::default(),
            count_run: None,
            secondary: None,
            glyphs: smallvec::SmallVec::new(),
            highlight_fffd: false,
            trailing: None,
        }
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
        _: &[u64],
        _: Option<SysColumnId>,
        menu: TlMenu,
        _: &mut Window,
        _: &mut Context<TlTreeTable<Self>>,
    ) -> TlMenu {
        menu
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
    fn on_visible_range(&mut self, _: &[u64], _: &mut Context<TlTreeTable<Self>>) {}
    fn on_activate(&mut self, _: u64, cx: &mut Context<TlTreeTable<Self>>) {
        cx.propagate();
    }
    fn row_tooltip(&self, _: usize) -> Option<AnyView> {
        None
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
        self.processes = Arc::clone(&snap.processes);
        self.rows = Arc::clone(&self.snap.rows);
        self.row_index = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, row)| (row_key(&row.node), ix))
            .collect();
        self.backend = self.snap.backend;
        self.version += 1;
    }
    fn mask(&self) -> ComponentMask {
        ComponentMask::SERVICES | ComponentMask::PROCESSES
    }
}
