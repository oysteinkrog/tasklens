//! The process row context menu and the group header menu (E22-A2, UI-PRC-013, UI-PRC-020,
//! PROC-057, PROC-066, R-033, R-034, R-091).
//!
//! The menu is built when it opens, from the targets captured at that moment, so a re-sort or
//! a new snapshot before the click does not change what an item acts on. Every item is one
//! Action dispatched in the ProcessTable context, so the key hints come from its bindings.
//!
//! Later epics add their items by chain edit: one `add_<item>` helper each, called in item
//! order from [`row_menu`]. Items of a phase that has not shipped are not built.

use gpui_kit::{App, SharedString, Window, px};
use tl_model::ids::NodeId;

use super::actions::{
    CapturedTargets, CopyAllMetrics, CopyDetails, EndProcess, EndProcessTree, EndTask, GoToParent,
    OpenFileLocation, PriorityClass, ResumeProcess, SetPriority, SuspendProcess, ToggleGroupRow,
};
use super::{SuspendLabel, suspend_or_resume};
use crate::components::menu::TlMenu;
use crate::i18n::tr;
use crate::table::actions::{CopyCell, CopyRowsAsTable};
use crate::table::tree_table::RowKind;

/// Minimum width of the row and group header menus before the UI scale (R-112).
pub const ROW_MENU_MIN_W: f32 = 240.0;

/// What the desktop offers, for the items that need it. Every field is false until the bead
/// that ships such an item fills it; no P1 item reads one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuCaps {
    pub kwin_bridge: bool,
    pub file_manager1: bool,
    pub gdb: bool,
    pub helper_session: bool,
}

/// The classes of the Set Priority submenu in P1, top to bottom (UI-PRC-020).
const PRIORITY_CLASSES: [(PriorityClass, &str); 5] = [
    (PriorityClass::High, "processes-menu-priority-high"),
    (
        PriorityClass::AboveNormal,
        "processes-menu-priority-above-normal",
    ),
    (PriorityClass::Normal, "processes-menu-priority-normal"),
    (
        PriorityClass::BelowNormal,
        "processes-menu-priority-below-normal",
    ),
    (PriorityClass::Low, "processes-menu-priority-low"),
];

/// Which row-type rules apply to one menu.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// Two or more targets, any mix.
    multi: bool,
    /// One app or unit row.
    group: bool,
    /// One process or kernel-thread row.
    process: bool,
    /// Every target is a pseudo row.
    pseudo: bool,
    /// Some target has exited: everything but the copy items is disabled.
    exited: bool,
}

impl Shape {
    fn of(t: &CapturedTargets) -> Self {
        let multi = t.is_multi();
        let kind = t.items.first().map(|t| t.kind);
        Self {
            multi,
            group: !multi && matches!(kind, Some(RowKind::App | RowKind::Unit)),
            process: !multi && matches!(kind, Some(RowKind::Process | RowKind::KernelThread)),
            pseudo: !t.items.is_empty() && t.items.iter().all(|t| t.kind == RowKind::Pseudo),
            exited: t.any_exited(),
        }
    }
}

/// The row menu for `targets` (UI-PRC-013). Pseudo rows get only Copy, Copy details and Copy
/// all metrics.
pub fn row_menu(
    menu: TlMenu,
    targets: &CapturedTargets,
    _caps: &MenuCaps,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let shape = Shape::of(targets);
    let mut menu = menu.min_w(px(ROW_MENU_MIN_W));
    if shape.pseudo {
        menu = add_copy(menu, targets, cx);
        menu = add_copy_details(menu, cx);
        return add_copy_all_metrics(menu, cx);
    }
    menu = add_end_task(menu, shape, cx);
    menu = add_end_process(menu, shape, cx);
    menu = add_end_process_tree(menu, shape, cx);
    menu = menu.separator();
    menu = add_suspend(menu, targets, shape, cx);
    menu = add_set_priority(menu, targets, shape, window, cx);
    menu = menu.separator();
    menu = add_copy(menu, targets, cx);
    menu = add_copy_details(menu, cx);
    menu = add_copy_all_metrics(menu, cx);
    menu = add_copy_rows_as_table(menu, cx);
    menu = menu.separator();
    menu = add_open_file_location(menu, targets, shape, cx);
    menu = menu.separator();
    add_go_to_parent(menu, targets, shape, cx)
}

/// The group header menu: one item, "Collapse" on an expanded group, else "Expand". It acts on
/// the header `id`, not on the focus row.
pub fn group_header_menu(menu: TlMenu, expanded: bool, id: NodeId, cx: &App) -> TlMenu {
    let key = if expanded {
        "processes-menu-collapse"
    } else {
        "processes-menu-expand"
    };
    menu.min_w(px(ROW_MENU_MIN_W))
        .item(tr(cx, key), Box::new(ToggleGroupRow { node: id }))
}

/// The Set Priority submenu (UI-PRC-020): the autogroup note, then the five P1 classes. The
/// current class has a check mark; with several targets only when every one shares it.
pub fn priority_submenu(
    menu: TlMenu,
    targets: &CapturedTargets,
    _window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let current = current_class(targets);
    let mut menu = menu.header(tr(cx, "processes-menu-autogroup-note"));
    for (class, key) in PRIORITY_CLASSES {
        menu = menu.check_item(
            tr(cx, key),
            current == Some(class),
            Box::new(SetPriority { class }),
        );
    }
    menu
}

/// The class whose nice value every target has. `None` when a target has no nice value (app,
/// unit and pseudo rows), when they differ, or when the value is not one of the P1 classes.
fn current_class(t: &CapturedTargets) -> Option<PriorityClass> {
    let mut nices = t.items.iter().map(|t| t.nice);
    let first = nices.next()??;
    if !nices.all(|n| n == Some(first)) {
        return None;
    }
    PRIORITY_CLASSES
        .iter()
        .map(|(class, _)| *class)
        .find(|class| class.nice() == Some(first))
}

/// An item that is disabled with "Process exited" on an exited row.
fn exitable(
    menu: TlMenu,
    shape: Shape,
    label: SharedString,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    if shape.exited {
        menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")))
    } else {
        menu.item(label, action)
    }
}

/// Item 1: End Task, End App or End Selected Tasks.
fn add_end_task(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    let key = if shape.multi {
        "processes-menu-end-selected-tasks"
    } else if shape.group {
        "processes-menu-end-app"
    } else {
        "processes-menu-end-task"
    };
    exitable(menu, shape, tr(cx, key), Box::new(EndTask), cx)
}

/// Item 2: End Process or End Selected Processes. Not on app and unit rows in P1.
fn add_end_process(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    if shape.group {
        return menu;
    }
    let key = if shape.multi {
        "processes-menu-end-selected-processes"
    } else {
        "processes-menu-end-process"
    };
    let menu = exitable(menu, shape, tr(cx, key), Box::new(EndProcess), cx);
    if shape.exited {
        menu
    } else {
        menu.tooltip(tr(cx, "processes-menu-end-process-tip"))
    }
}

/// Item 3: End Process Tree, on one process row only.
fn add_end_process_tree(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    if !shape.process {
        return menu;
    }
    let label = tr(cx, "processes-menu-end-process-tree");
    exitable(menu, shape, label, Box::new(EndProcessTree), cx)
}

/// Item 4: Suspend or Resume, by State T. Not on app and unit rows in P1.
fn add_suspend(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    if shape.group {
        return menu;
    }
    let resume = suspend_or_resume(targets) == SuspendLabel::Resume;
    let (key, action): (&str, Box<dyn gpui_kit::Action>) = match (resume, shape.multi) {
        (false, false) => ("processes-menu-suspend-process", Box::new(SuspendProcess)),
        (false, true) => (
            "processes-menu-suspend-selected-tasks",
            Box::new(SuspendProcess),
        ),
        (true, false) => ("processes-menu-resume-process", Box::new(ResumeProcess)),
        (true, true) => (
            "processes-menu-resume-selected-tasks",
            Box::new(ResumeProcess),
        ),
    };
    exitable(menu, shape, tr(cx, key), action, cx)
}

/// Item 6: the Set Priority submenu. On an app or unit row it says it applies to the app's
/// current processes.
fn add_set_priority(
    menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let label = tr(cx, "processes-menu-set-priority");
    if shape.exited {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")));
    }
    let targets = targets.clone();
    let menu = menu.submenu(label, window, cx, move |sub, window, cx| {
        priority_submenu(sub, &targets, window, cx)
    });
    if shape.group {
        menu.tooltip(tr(cx, "processes-menu-set-priority-app-tip"))
    } else {
        menu
    }
}

/// Item 11: Copy, for the column under the pointer. Not built from the keyboard.
fn add_copy(menu: TlMenu, targets: &CapturedTargets, cx: &App) -> TlMenu {
    match targets.clicked_col {
        Some(col) => menu.item(tr(cx, "processes-menu-copy"), Box::new(CopyCell { col })),
        None => menu,
    }
}

/// Item 12: Copy details.
fn add_copy_details(menu: TlMenu, cx: &App) -> TlMenu {
    menu.item(tr(cx, "processes-menu-copy-details"), Box::new(CopyDetails))
}

/// Item 13: Copy all metrics.
fn add_copy_all_metrics(menu: TlMenu, cx: &App) -> TlMenu {
    menu.item(
        tr(cx, "processes-menu-copy-all-metrics"),
        Box::new(CopyAllMetrics),
    )
}

/// Item 14: Copy rows as table.
fn add_copy_rows_as_table(menu: TlMenu, cx: &App) -> TlMenu {
    menu.item(
        tr(cx, "processes-menu-copy-rows-as-table"),
        Box::new(CopyRowsAsTable),
    )
}

/// Item 21: Open file location, on one process row. Disabled when the executable was deleted
/// (E22-020).
fn add_open_file_location(
    menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    cx: &App,
) -> TlMenu {
    if !shape.process {
        return menu;
    }
    let label = tr(cx, "processes-menu-open-file-location");
    if !shape.exited && targets.items.iter().any(|t| t.exe_deleted) {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-executable-deleted")));
    }
    exitable(menu, shape, label, Box::new(OpenFileLocation), cx)
}

/// Item 24: Go to parent process, on one process row whose parent PID is not 0.
fn add_go_to_parent(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    if !shape.process || targets.items.iter().all(|t| t.ppid == 0) {
        return menu;
    }
    let label = tr(cx, "processes-menu-go-to-parent");
    exitable(menu, shape, label, Box::new(GoToParent), cx)
}
