//! Connections row context menu (UI-SYS-063, R-012, R-091).
//!
//! Targets contain record values, not row indexes. A refresh may therefore replace or reorder
//! the table while the menu is open without changing what the chosen command acts on.

use std::sync::Arc;

use gpui_kit::{App, Window, px};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;
use tl_model::actions::{ActionTarget, Family, FourTuple, Proto};
use tl_model::health::HelperState;
use tl_model::ids::{ColumnId as ActionColumnId, ProcessKey};
use tl_model::snapshot::{ConnRecord, ConnState};
use tl_model::table::{SysColumnId, UnitRef};
use tl_model::validity::Validity;

use super::actions::{
    ConnectionCloseTcp, ConnectionCopyForeign, ConnectionCopyLocal, ConnectionGoToService,
    ConnectionJumpToProcess,
};
use crate::components::menu::TlMenu;
use crate::hub::GlobalHub;
use crate::i18n::tr;
use crate::pages::processes::actions::CopyDetails;
use crate::table::actions::{CopyCell, CopyRowsAsTable, OpenProperties};

/// The column type stored for a Connections menu cell.
pub type ColumnId = SysColumnId;

/// Values one Connections command may need after the snapshot that supplied the row is gone.
#[derive(Clone, Debug)]
#[allow(dead_code)] // The following E28 beads consume these captured handler values.
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

impl ConnRecordRef {
    /// True for a TCP socket that is not in TIME-WAIT: the only kind "Close TCP connection" acts
    /// on (UI-SYS-064).
    pub fn closable(&self) -> bool {
        self.proto == Proto::Tcp && self.state != ConnState::TimeWait
    }

    /// The request target, built from the values captured at menu open only. The helper matches
    /// the cookie and the four-tuple, so a reused four-tuple is never hit (R-119).
    pub fn socket_target(&self) -> ActionTarget {
        ActionTarget::Socket {
            cookie: self.cookie,
            family: self.family,
            proto: self.proto,
            four_tuple: self.four_tuple,
        }
    }
}

/// How the "Close TCP connection" item shows (UI-SYS-064), first match wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseItem {
    /// Every target is UDP or TIME-WAIT.
    Hidden,
    /// tl-helper is not installed.
    NeedsHelper,
    /// The kernel answered EOPNOTSUPP earlier this session.
    NoKernelSupport,
    /// A close is pending on every closable target.
    Closing,
    Enabled,
}

/// The item state for `targets`. `closing` holds the cookies with a close in flight.
pub fn close_item(
    targets: &ConnMenuTargets,
    helper: HelperState,
    destroy_supported: Option<bool>,
    closing: &FxHashSet<u64>,
) -> CloseItem {
    let mut closable = targets.records.iter().filter(|r| r.closable()).peekable();
    if closable.peek().is_none() {
        CloseItem::Hidden
    } else if helper == HelperState::Missing {
        CloseItem::NeedsHelper
    } else if destroy_supported == Some(false) {
        CloseItem::NoKernelSupport
    } else if closable.all(|r| closing.contains(&r.cookie)) {
        CloseItem::Closing
    } else {
        CloseItem::Enabled
    }
}

/// The helper state and the session's destroy support, from the hub's current snapshot.
fn close_inputs(cx: &App) -> (HelperState, Option<bool>) {
    cx.try_global::<GlobalHub>()
        .map_or((HelperState::Missing, None), |hub| {
            let snap = hub.0.read(cx).current();
            (snap.health.helper, snap.connections.destroy_supported)
        })
}

/// Connections rows and optional pointer cell captured when a row menu opens.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // The following E28 beads consume the captured cookie list.
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

/// Builds the Connections row menu in UI-SYS-063 order. `closing` holds the cookies with a
/// close in flight.
pub fn row_menu(
    mut menu: TlMenu,
    targets: &ConnMenuTargets,
    closing: &FxHashSet<u64>,
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

    let (helper, destroy_supported) = close_inputs(cx);
    let close = close_item(targets, helper, destroy_supported, closing);
    if close != CloseItem::Hidden {
        let tooltip = match close {
            CloseItem::NeedsHelper => Some(tr(cx, "connections-close-needs-helper")),
            CloseItem::NoKernelSupport => Some(tr(cx, "connections-close-no-kernel-support")),
            CloseItem::Closing => Some(tr(cx, "connections-close-closing")),
            CloseItem::Hidden | CloseItem::Enabled => None,
        };
        menu = item_enabled(
            menu,
            "connections-close-menu",
            close == CloseItem::Enabled,
            tooltip,
            Box::new(ConnectionCloseTcp),
            cx,
        );
        menu = menu.separator();
    }

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
