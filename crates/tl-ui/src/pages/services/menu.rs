//! Services unit row context menu and its Startup type submenu (UI-SYS-017, UI-SYS-019,
//! SYS-006 to SYS-010, R-012, R-091, ADR-009).
//!
//! Targets hold unit references and record values captured when the menu opens. A refresh may
//! re-sort or replace the table while the menu is open without changing what a command acts on.

use gpui_kit::{App, SharedString, Window, px};
use smallvec::SmallVec;
use tl_model::actions::ActionTarget;
use tl_model::snapshot::{
    ServiceBackend, ServiceStatus, ServicesSnapshot, StartupType, UnitRecord,
};
use tl_model::table::{SysColumnId, SysNodeId, UnitRef, UnitType};

use super::actions::{
    ServiceFileProperties, ServiceJumpToProcess, ServiceOpenFileLocation, ServiceOpenUnitFile,
    ServiceReload, ServiceResetFailed, ServiceRestart, ServiceSetStartupType, ServiceStart,
    ServiceStop, ServiceViewLogs, StartupChoice,
};
use super::copy::column_alias;
use crate::components::menu::TlMenu;
use crate::i18n::tr;
use crate::pages::processes::actions::CopyDetails;
use crate::pages::sys_table::SysActions;
use crate::table::actions::{CopyCell, CopyRowsAsTable, OpenProperties};

/// Most unit rows one menu acts on.
const MAX_TARGETS: usize = 256;

/// Captured at menu open; every item acts on these, never on the live selection.
#[derive(Clone, Debug, Default)]
pub struct ServiceMenuTargets {
    /// Unit rows of the selection, in selection order.
    pub units: SmallVec<[UnitRef; 1]>,
    /// Their records at open time.
    pub records: SmallVec<[UnitRecord; 1]>,
    /// For each record: a service action is in flight on it.
    pub pending: SmallVec<[bool; 1]>,
    /// The cell under the pointer. `None` for a keyboard open, which has no cell Copy.
    pub clicked_col: Option<SysColumnId>,
}

impl ServiceMenuTargets {
    /// Keeps the unit rows of `selection` (at most 256) that `services` still lists. Child
    /// process rows and group rows are ignored.
    pub fn capture(
        services: &ServicesSnapshot,
        selection: &[SysNodeId],
        clicked_col: Option<SysColumnId>,
        actions: &SysActions,
    ) -> Self {
        let mut out = Self {
            clicked_col,
            ..Self::default()
        };
        for node in selection {
            let SysNodeId::Unit(unit) = node else {
                continue;
            };
            if out.units.len() >= MAX_TARGETS {
                break;
            }
            let Some(record) = services.units.iter().find(|r| &r.unit == unit) else {
                continue;
            };
            out.pending.push(
                actions
                    .pending_tag(&ActionTarget::Unit(unit.clone()))
                    .is_some(),
            );
            out.units.push(unit.clone());
            out.records.push(record.clone());
        }
        out
    }

    fn is_one(&self) -> bool {
        self.records.len() == 1
    }

    fn any_pending(&self) -> bool {
        self.pending.iter().any(|p| *p)
    }
}

/// Which menu a right-click gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuChoice {
    Unit,
    /// The page opens the process row menu.
    Process,
    None,
}

/// A mixed selection gets the unit menu, which ignores the child rows. Group rows have no menu.
pub fn menu_choice(selection: &[SysNodeId]) -> MenuChoice {
    if selection.iter().any(|n| matches!(n, SysNodeId::Unit(_))) {
        MenuChoice::Unit
    } else if selection
        .iter()
        .any(|n| matches!(n, SysNodeId::UnitProcess(..)))
    {
        MenuChoice::Process
    } else {
        MenuChoice::None
    }
}

/// ActiveState `active` or `reloading`, the set UI-SYS-022 counts as Running.
fn is_active(record: &UnitRecord) -> bool {
    matches!(
        record.status,
        ServiceStatus::Running | ServiceStatus::Active | ServiceStatus::Reloading
    )
}

fn has_binary(record: &UnitRecord) -> bool {
    record.kind == UnitType::Service && !record.path.is_empty()
}

fn has_unit_file(record: &UnitRecord) -> bool {
    record.unit_file.as_deref().is_some_and(|p| !p.is_empty())
}

fn startup_choice(startup: StartupType) -> Option<StartupChoice> {
    match startup {
        StartupType::Enabled | StartupType::EnabledRuntime => Some(StartupChoice::Enabled),
        StartupType::Disabled => Some(StartupChoice::Disabled),
        StartupType::Masked => Some(StartupChoice::Masked),
        _ => None,
    }
}

/// Builds the Services unit row menu in UI-SYS-017 order.
pub fn service_menu(
    menu: TlMenu,
    t: &ServiceMenuTargets,
    backend: ServiceBackend,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let busy = t.any_pending();
    let busy_tip = || busy.then(|| tr(cx, "services-sys017-action-in-progress"));
    let any_idle = |active: bool| {
        t.records
            .iter()
            .zip(&t.pending)
            .any(|(r, pending)| is_active(r) == active && !pending)
    };

    let mut menu = menu.min_w(px(220.));
    menu = item_enabled(
        menu,
        "services-sys017-start",
        any_idle(false),
        busy_tip(),
        Box::new(ServiceStart),
        cx,
    );
    menu = item_enabled(
        menu,
        "services-sys017-stop",
        any_idle(true),
        busy_tip(),
        Box::new(ServiceStop),
        cx,
    );
    menu = item_enabled(
        menu,
        "services-sys017-restart",
        any_idle(true),
        busy_tip(),
        Box::new(ServiceRestart),
        cx,
    );
    let openrc = backend == ServiceBackend::OpenRc;
    if !openrc && t.records.iter().any(|r| r.can_reload) {
        let enabled = t.records.iter().any(|r| r.can_reload && is_active(r));
        menu = item_enabled(
            menu,
            "services-sys017-reload",
            enabled,
            None,
            Box::new(ServiceReload),
            cx,
        );
    }
    if !openrc && t.records.iter().any(|r| r.status == ServiceStatus::Failed) {
        menu = menu.item(
            tr(cx, "services-sys017-reset-failed"),
            Box::new(ServiceResetFailed),
        );
    }
    menu = startup_item(menu, t, backend, window, cx);
    menu = menu.separator();

    if !openrc {
        menu = item_enabled(
            menu,
            "services-sys017-view-logs",
            t.is_one(),
            None,
            Box::new(ServiceViewLogs),
            cx,
        );
    }
    menu = menu.item(
        tr(cx, "services-sys017-properties"),
        Box::new(OpenProperties),
    );
    let binary = t.records.iter().any(has_binary);
    let image_tip = (!binary).then(|| tr(cx, "services-sys017-image-unavailable"));
    menu = item_enabled(
        menu,
        "services-sys017-file-properties",
        binary || t.records.iter().any(has_unit_file),
        image_tip.clone(),
        Box::new(ServiceFileProperties),
        cx,
    );
    menu = item_enabled(
        menu,
        "services-sys017-open-file-location",
        binary,
        image_tip,
        Box::new(ServiceOpenFileLocation),
        cx,
    );
    if !openrc {
        menu = item_enabled(
            menu,
            "services-sys017-open-unit-file",
            t.records.iter().any(has_unit_file),
            None,
            Box::new(ServiceOpenUnitFile),
            cx,
        );
    }
    menu = menu.separator();

    if let Some(SysColumnId::Services(col)) = t.clicked_col {
        menu = menu.item(
            tr(cx, "services-sys017-copy"),
            Box::new(CopyCell {
                col: column_alias(col),
            }),
        );
    }
    menu = menu.item(tr(cx, "services-sys017-copy-row"), Box::new(CopyDetails));
    menu = menu.item(
        tr(cx, "services-sys017-copy-rows-table"),
        Box::new(CopyRowsAsTable),
    );
    if openrc {
        return menu;
    }
    let (jump_enabled, jump_tip) = match t.records.as_slice() {
        [r] if r.main_pid != 0 => (true, None),
        [_] => (false, Some(tr(cx, "services-sys017-not-running"))),
        _ => (false, None),
    };
    item_enabled(
        menu,
        "services-sys017-jump-to-process",
        jump_enabled,
        jump_tip,
        Box::new(ServiceJumpToProcess),
        cx,
    )
}

/// The "Startup type" parent: a submenu, or a disabled row that says why not (UI-SYS-019).
fn startup_item(
    menu: TlMenu,
    t: &ServiceMenuTargets,
    backend: ServiceBackend,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let openrc = backend == ServiceBackend::OpenRc;
    let label = tr(cx, "services-sys019-startup-type");
    let no_install = t.records.iter().any(|r| {
        matches!(
            r.startup,
            StartupType::Static
                | StartupType::Indirect
                | StartupType::Generated
                | StartupType::Transient
        )
    });
    if no_install {
        return menu.item_disabled(label, Some(tr(cx, "services-sys019-no-install-section")));
    }
    if t.records
        .iter()
        .any(|r| r.startup == StartupType::NoUnitFile)
    {
        return menu.item_disabled(label, Some(tr(cx, "services-sys019-no-unit-file")));
    }
    // Checked only when every target maps to the same value. Alias and Linked map to none.
    let mut values = t.records.iter().map(|r| startup_choice(r.startup));
    let current = values
        .next()
        .flatten()
        .filter(|first| values.all(|v| v.as_ref() == Some(first)));
    menu.submenu(label, window, cx, move |sub, _, cx| {
        let mut sub = sub;
        for (key, choice) in [
            ("services-sys019-enabled", StartupChoice::Enabled),
            ("services-sys019-disabled", StartupChoice::Disabled),
            ("services-sys019-masked", StartupChoice::Masked),
        ] {
            if openrc && choice == StartupChoice::Masked {
                continue;
            }
            let checked = current.as_ref() == Some(&choice);
            sub = sub.check_item(
                tr(cx, key),
                checked,
                Box::new(ServiceSetStartupType { startup: choice }),
            );
        }
        sub
    })
}

fn item_enabled(
    menu: TlMenu,
    key: &str,
    enabled: bool,
    tooltip: Option<SharedString>,
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
