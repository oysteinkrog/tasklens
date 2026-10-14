//! Connections row context menu (UI-SYS-063, R-012, R-091).
//!
//! Targets contain record values, not row indexes. A refresh may therefore replace or reorder
//! the table while the menu is open without changing what the chosen command acts on.

use std::sync::Arc;

use gpui_kit::{App, Window, px};
use smallvec::SmallVec;
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::ids::{ColumnId as ActionColumnId, ProcessKey};
use tl_model::snapshot::{ConnRecord, ConnState};
use tl_model::table::{SysColumnId, UnitRef};
use tl_model::validity::Validity;

use super::actions::{
    ConnectionCopyForeign, ConnectionCopyLocal, ConnectionGoToService, ConnectionJumpToProcess,
};
use crate::components::menu::TlMenu;
use crate::i18n::tr;
use crate::pages::processes::actions::CopyDetails;
use crate::table::actions::{CopyCell, CopyRowsAsTable, OpenProperties};

/// The column type stored for a Connections menu cell.
pub type ColumnId = SysColumnId;

/// Values one Connections command may need after the snapshot that supplied the row is gone.
#[derive(Clone, Debug)]
pub struct ConnRecordRef {
    pub cookie: u64,
    pub proto: Proto,
    pub family: Family,
    pub four_tuple: FourTuple,
    pub state: ConnState,
    pub owners: SmallVec<[ProcessKey; 1]>,
    pub owner_validity: Validity,
    pub unit: Option<UnitRef>,
    pub local: Arc<str>,
    pub foreign: Arc<str>,
}

impl From<&ConnRecord> for ConnRecordRef {
    fn from(record: &ConnRecord) -> Self {
        Self {
            cookie: record.cookie,
            proto: record.proto,
            family: record.family,
            four_tuple: record.four_tuple,
            state: record.state,
            owners: record.owners.clone(),
            owner_validity: record.owner_validity,
            unit: record.unit.clone(),
            local: Arc::clone(&record.local),
            foreign: Arc::clone(&record.foreign),
        }
    }
}

/// Connections rows and optional pointer cell captured when a row menu opens.
#[derive(Clone, Debug, Default)]
pub struct ConnMenuTargets {
    pub cookies: SmallVec<[u64; 1]>,
    pub records: SmallVec<[ConnRecordRef; 1]>,
    pub cell: Option<(u64, ColumnId)>,
}

impl ConnMenuTargets {
    pub fn is_one(&self) -> bool {
        self.records.len() == 1
    }
}

/// Builds the P2 Connections row menu in UI-SYS-063 order.
pub fn row_menu(
    mut menu: TlMenu,
    targets: &ConnMenuTargets,
    _window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    menu = menu.min_w(px(220.));
    if let Some((_, col)) = targets.cell {
        // `CopyCell` predates the system-table column ids. The Connections handler reads the
        // authoritative system column from `ConnMenuTargets::cell`.
        menu = menu.item(
            tr(cx, "connections-menu-copy"),
            Box::new(CopyCell {
                col: action_column(col),
            }),
        );
    }
    menu = menu.item(tr(cx, "connections-menu-copy-row"), Box::new(CopyDetails));
    menu = menu.item(
        tr(cx, "connections-menu-copy-rows-table"),
        Box::new(CopyRowsAsTable),
    );
    menu = enabled_for_one(
        menu,
        targets,
        "connections-menu-copy-local",
        Box::new(ConnectionCopyLocal),
        cx,
    );
    let foreign_enabled = targets.is_one() && targets.records[0].foreign.as_ref() != "*:*";
    menu = item_enabled(
        menu,
        "connections-menu-copy-foreign",
        foreign_enabled,
        None,
        Box::new(ConnectionCopyForeign),
        cx,
    );
    menu = menu.separator();

    let (jump_enabled, jump_tip) = match targets.records.as_slice() {
        [record] if record.owner_validity == Validity::Restricted => {
            (false, Some(tr(cx, "connections-menu-needs-administrator")))
        }
        [record] if !record.owners.is_empty() => (true, None),
        [_] => (false, Some(tr(cx, "connections-menu-no-owning-process"))),
        _ => (false, None),
    };
    menu = item_enabled(
        menu,
        "connections-menu-jump-process",
        jump_enabled,
        jump_tip,
        Box::new(ConnectionJumpToProcess),
        cx,
    );
    if targets
        .records
        .as_slice()
        .first()
        .is_some_and(|record| targets.is_one() && record.unit.is_some())
    {
        menu = menu.item(
            tr(cx, "connections-menu-go-service"),
            Box::new(ConnectionGoToService),
        );
    }
    menu.item(
        tr(cx, "connections-menu-properties"),
        Box::new(OpenProperties),
    )
}

fn enabled_for_one(
    menu: TlMenu,
    targets: &ConnMenuTargets,
    key: &str,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    item_enabled(menu, key, targets.is_one(), None, action, cx)
}

fn item_enabled(
    menu: TlMenu,
    key: &str,
    enabled: bool,
    tooltip: Option<gpui_kit::SharedString>,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    let label = tr(cx, key);
    if enabled {
        menu.item(label, action)
    } else {
        menu.item_disabled(label, tooltip)
    }
}

/// Compatibility payload for the shared action. The captured system column above is the value
/// the Connections copy handler uses.
fn action_column(col: SysColumnId) -> ActionColumnId {
    use tl_model::table::ConnColumn;

    match col {
        SysColumnId::Connections(ConnColumn::Pid) => ActionColumnId::Pid,
        SysColumnId::Connections(ConnColumn::Process) => ActionColumnId::Name,
        SysColumnId::Connections(ConnColumn::User) => ActionColumnId::User,
        SysColumnId::Connections(ConnColumn::State) => ActionColumnId::State,
        SysColumnId::Connections(ConnColumn::Service) => ActionColumnId::Unit,
        _ => ActionColumnId::Network,
    }
}
