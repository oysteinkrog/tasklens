//! Startup Apps row context menu (UI-SYS-044, R-012, R-091).
//!
//! Targets hold entry values, not row indexes. A rescan may reorder or replace the table while
//! the menu is open without changing what the chosen command acts on.

use gpui_kit::{App, SharedString, Window, px};
use smallvec::SmallVec;
use tl_model::ids::ColumnId as ActionColumnId;
use tl_model::snapshot::{StartupEntry, StartupKind, StartupStatus};
use tl_model::table::{StartupColumn, StartupRef, SysColumnId};

use super::actions::{
    StartupCopyCommand, StartupDelete, StartupDisable, StartupEnable, StartupOpenFileLocation,
    StartupRunNow,
};
use crate::components::menu::TlMenu;
use crate::host::HostContext;
use crate::i18n::tr;
use crate::pages::processes::actions::CopyDetails;
use crate::table::actions::{CopyCell, CopyRowsAsTable, OpenProperties};

/// Rows and the optional pointer cell captured when a row menu opens.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // The following E28 beads read the captured entries and cell.
pub struct StartupMenuTargets {
    pub refs: SmallVec<[StartupRef; 1]>,
    /// Clones of the entries taken at open time, in the same order as `refs`.
    pub entries: SmallVec<[StartupEntry; 1]>,
    pub cell: Option<(StartupRef, SysColumnId)>,
}

impl StartupMenuTargets {
    pub fn is_one(&self) -> bool {
        self.refs.len() == 1
    }

    /// True when at least one captured entry may be deleted.
    pub fn any_can_delete(&self) -> bool {
        self.entries.iter().any(|entry| entry.can_delete)
    }
}

/// Builds the Startup row menu in UI-SYS-044 order.
pub fn row_menu(
    mut menu: TlMenu,
    targets: &StartupMenuTargets,
    host: &HostContext,
    _window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    menu = menu.min_w(px(220.));
    let entries = targets.entries.as_slice();
    let any_enabled = entries.iter().any(|e| e.status == StartupStatus::Enabled);
    let any_other = entries.iter().any(|e| e.status != StartupStatus::Enabled);

    if any_enabled {
        let enabled = entries
            .iter()
            .any(|e| e.status == StartupStatus::Enabled && e.can_toggle);
        let tip = (!enabled)
            .then(|| {
                toggle_tip(
                    entries
                        .iter()
                        .filter(|e| e.status == StartupStatus::Enabled),
                    cx,
                )
            })
            .flatten();
        menu = item_enabled(
            menu,
            "startup-menu-disable",
            enabled,
            tip,
            Box::new(StartupDisable),
            cx,
        );
    }
    if any_other {
        let enabled = entries
            .iter()
            .any(|e| e.status != StartupStatus::Enabled && e.can_toggle);
        let tip = (!enabled)
            .then(|| {
                toggle_tip(
                    entries
                        .iter()
                        .filter(|e| e.status != StartupStatus::Enabled),
                    cx,
                )
            })
            .flatten();
        menu = item_enabled(
            menu,
            "startup-menu-enable",
            enabled,
            tip,
            Box::new(StartupEnable),
            cx,
        );
    }
    let run_enabled = entries.iter().any(|e| e.can_run);
    let run_tip = (!run_enabled)
        .then(|| toggle_tip(entries.iter(), cx))
        .flatten();
    menu = desktop_item_enabled(
        host,
        menu,
        "startup-menu-run-now",
        run_enabled,
        run_tip,
        Box::new(StartupRunNow),
        cx,
    );
    menu = menu.separator();

    if let Some((_, col)) = &targets.cell {
        menu = menu.item(
            tr(cx, "startup-menu-copy"),
            Box::new(CopyCell {
                col: action_column(*col),
            }),
        );
    }
    menu = menu.item(tr(cx, "startup-menu-copy-row"), Box::new(CopyDetails));
    menu = menu.item(
        tr(cx, "startup-menu-copy-rows-table"),
        Box::new(CopyRowsAsTable),
    );
    menu = item_enabled(
        menu,
        "startup-menu-copy-command",
        targets.is_one(),
        None,
        Box::new(StartupCopyCommand),
        cx,
    );
    menu = desktop_item_enabled(
        host,
        menu,
        "startup-menu-open-location",
        targets.is_one(),
        None,
        Box::new(StartupOpenFileLocation),
        cx,
    );
    menu = menu.item(tr(cx, "startup-menu-properties"), Box::new(OpenProperties));
    menu = menu.separator();

    let delete_enabled = targets.any_can_delete();
    let delete_tip = (!delete_enabled).then(|| delete_tip(entries, cx)).flatten();
    item_enabled(
        menu,
        "startup-menu-delete",
        delete_enabled,
        delete_tip,
        Box::new(StartupDelete),
        cx,
    )
}

fn item_enabled(
    menu: TlMenu,
    key: &str,
    enabled: bool,
    tooltip: Option<SharedString>,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    item_labeled(menu, tr(cx, key), enabled, tooltip, action)
}

/// [`item_enabled`] for an item that acts on the host's desktop: a remote host's label ends
/// with "on <host>" (UI-SHL-008).
fn desktop_item_enabled(
    host: &HostContext,
    menu: TlMenu,
    key: &str,
    enabled: bool,
    tooltip: Option<SharedString>,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    let label = host.desktop_label(tr(cx, key), cx);
    item_labeled(menu, label, enabled, tooltip, action)
}

fn item_labeled(
    menu: TlMenu,
    label: SharedString,
    enabled: bool,
    tooltip: Option<SharedString>,
    action: Box<dyn gpui_kit::Action>,
) -> TlMenu {
    if enabled {
        menu.item(label, action)
    } else {
        menu.item_disabled(label, tooltip)
    }
}

/// The reason text of the first entry whose status blocks a toggle or a run.
fn toggle_tip<'a>(
    entries: impl Iterator<Item = &'a StartupEntry>,
    cx: &App,
) -> Option<SharedString> {
    entries.into_iter().find_map(|entry| match entry.status {
        StartupStatus::NotInThisDesktop => Some(tr(cx, "startup-menu-reason-not-in-desktop")),
        StartupStatus::MissingProgram => Some(tr(cx, "startup-menu-reason-missing-program")),
        StartupStatus::Masked => Some(tr(cx, "startup-menu-reason-masked")),
        StartupStatus::Enabled | StartupStatus::Disabled => None,
    })
}

/// The reason text of the first entry that cannot be deleted.
fn delete_tip(entries: &[StartupEntry], cx: &App) -> Option<SharedString> {
    entries
        .iter()
        .find(|entry| !entry.can_delete)
        .and_then(|entry| match entry.kind {
            StartupKind::AutostartSystem | StartupKind::AutostartSystemOverridden => {
                Some(tr(cx, "startup-menu-reason-system-entry"))
            }
            StartupKind::UserUnit => Some(tr(cx, "startup-menu-reason-package-unit")),
            _ => None,
        })
}

/// Compatibility payload for the shared `CopyCell` action. The captured system column in
/// `StartupMenuTargets::cell` is the value the Startup copy handler uses.
fn action_column(col: SysColumnId) -> ActionColumnId {
    match col {
        SysColumnId::Startup(StartupColumn::Name | StartupColumn::Impact) => ActionColumnId::Name,
        SysColumnId::Startup(StartupColumn::Type) => ActionColumnId::Type,
        SysColumnId::Startup(StartupColumn::Command) => ActionColumnId::Cmdline,
        SysColumnId::Startup(StartupColumn::Publisher) => ActionColumnId::Company,
        SysColumnId::Startup(StartupColumn::Location) => ActionColumnId::ImagePath,
        SysColumnId::Startup(StartupColumn::Status) => ActionColumnId::State,
        _ => ActionColumnId::Name,
    }
}
