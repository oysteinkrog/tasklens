//! The process row context menu and the group header menu (E22-A2, UI-PRC-013, UI-PRC-020,
//! PROC-057, PROC-066, R-033, R-034, R-091).
//!
//! The menu is built when it opens, from the targets captured at that moment, so a re-sort or
//! a new snapshot before the click does not change what an item acts on. Every item is one
//! Action dispatched in the ProcessTable context, so the key hints come from its bindings.
//!
//! Later epics add their items by chain edit: one `add_<item>` helper each, called in item
//! order from [`row_menu`]. Items of a phase that has not shipped are not built.

use std::sync::Arc;

use fluent_bundle::FluentValue;
use gpui_kit::{App, SharedString, Window, px};
use tl_model::actions::SchedPolicy;
use tl_model::health::HelperState;
use tl_model::ids::NodeId;
use tl_model::process::{GroupKind, GroupRowKind};
use tl_model::snapshot::{ProcessSnapshot, Snapshot, WindowEntry};
use tl_model::table::Manager;
use tl_model::validity::Validity;

use super::actions::{
    ActivateWindow, CapturedTarget, CapturedTargets, CopyAllMetrics, CopyDetails, CreateDump,
    EndProcess, EndProcessTree, EndTask, FileProperties, FreezeApp, GoToParent, IoClass, KillApp,
    OpenFileLocation, OpenVirusTotal, PriorityClass, RestartUnit, ResumeProcess,
    SetIoPriorityClass, SetPriority, ShowWindow, SuspendProcess, ThawApp, ToggleGroupRow,
};
use super::{
    SuspendLabel, group_frozen, group_targets, plasma_restart_unit, suspend_or_resume,
    topmost_window,
};
use crate::app::EngineGlobal;
use crate::components::menu::TlMenu;
use crate::i18n::{tr, tr_args};
use crate::pages::minigraph::actions::{AddAppGraph, AddMinigraph, AddNameGraph};
use crate::pages::minigraph::panel::MinigraphPanel;
use crate::pages::minigraph::sources::{GraphColumn, MinigraphSource};
use crate::pages::services::actions::{GoToService, JumpOrigin, ManagerArg};
use crate::table::actions::{CopyCell, CopyRowsAsTable, OpenProperties};
use crate::table::tree_table::RowKind;
use crate::tray_bridge::{AddTrayGraph, RemoveTrayGraph, TrayBridge};
use crate::window::main::tray_available;
use crate::windows::actions::{OpenCustomPriority, SetAffinity};
use crate::windows::signal::signal_submenu;

/// Minimum width of the row and group header menus before the UI scale (R-112).
pub const ROW_MENU_MIN_W: f32 = 240.0;

/// Maximum width of the window button's popup before the UI scale (UI-PRC-003).
pub const WINDOW_POPUP_MAX_W: f32 = 480.0;

/// What the desktop offers, for the items that need it. Every field is false until the bead
/// that ships such an item fills it; no P1 item reads one. Set I/O Priority also reads the
/// helper state of the snapshot, so `helper_session` false is not the final word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuCaps {
    pub kwin_bridge: bool,
    pub file_manager1: bool,
    pub gdb: bool,
    pub helper_session: bool,
}

/// The classes of the Set Priority submenu, top to bottom (UI-PRC-020).
const PRIORITY_CLASSES: [(PriorityClass, &str); 7] = [
    (PriorityClass::Realtime, "processes-menu-priority-realtime"),
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
    (PriorityClass::Idle, "processes-menu-priority-idle"),
];

/// The classes of the Set I/O Priority submenu above its separator, top to bottom
/// (UI-PRC-020). "Default (follows nice)" follows the separator.
const IO_CLASSES: [(IoClass, &str); 5] = [
    (IoClass::Realtime, "processes-menu-io-realtime"),
    (IoClass::High, "processes-menu-io-high"),
    (IoClass::Normal, "processes-menu-io-normal"),
    (IoClass::Low, "processes-menu-io-low"),
    (IoClass::Idle, "processes-menu-io-idle"),
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
    caps: &MenuCaps,
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
    menu = add_end_process(menu, targets, shape, cx);
    menu = add_end_process_tree(menu, shape, cx);
    menu = menu.separator();
    menu = add_suspend(menu, targets, shape, cx);
    menu = add_send_signal(menu, targets, shape, window, cx);
    menu = add_set_priority(menu, targets, shape, window, cx);
    menu = add_set_affinity(menu, shape, cx);
    menu = add_set_io_priority(menu, targets, caps, shape, window, cx);
    menu = menu.separator();
    menu = add_show_window(menu, targets, caps, shape, cx);
    menu = add_restart_unit(menu, targets, shape, cx);
    menu = menu.separator();
    menu = add_copy(menu, targets, cx);
    menu = add_copy_details(menu, cx);
    menu = add_copy_all_metrics(menu, cx);
    menu = add_copy_rows_as_table(menu, cx);
    menu = menu.separator();
    menu = add_graph_menus(menu, targets, shape, window, cx);
    menu = add_tray_graph(menu, targets, shape, window, cx);
    menu = menu.separator();
    menu = add_properties(menu, shape, cx);
    menu = add_file_properties(menu, targets, caps, shape, cx);
    menu = add_open_file_location(menu, targets, shape, cx);
    menu = add_open_virustotal(menu, shape, cx);
    menu = add_create_dump(menu, caps, shape, cx);
    menu = menu.separator();
    menu = add_go_to_parent(menu, targets, shape, cx);
    add_go_to_service(menu, targets, shape, cx)
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

/// The Name-cell window button's popup (PROC-053, UI-PRC-003): one item per window in the order
/// given, labelled with its caption or "Untitled window", no icon. Each item dispatches
/// `ActivateWindow` with the window's KWin id. Captions are display only. The menu is 240 to
/// 480 px wide and ends a longer caption in an ellipsis.
pub fn window_popup(menu: TlMenu, windows: &[WindowEntry], cx: &App) -> TlMenu {
    let mut untitled: Option<SharedString> = None;
    windows.iter().fold(
        menu.min_w(px(ROW_MENU_MIN_W)).max_w(px(WINDOW_POPUP_MAX_W)),
        |menu, w| {
            let label = if w.caption.trim().is_empty() {
                untitled
                    .get_or_insert_with(|| tr(cx, "processes-name-untitled-window"))
                    .clone()
            } else {
                SharedString::from(Arc::clone(&w.caption))
            };
            let action = ActivateWindow {
                internal_id: SharedString::from(Arc::clone(&w.internal_id)),
            };
            menu.item(label, Box::new(action))
        },
    )
}

/// Widest Set Priority submenu at 100% scale: room for the autogroup note without an ellipsis.
const PRIORITY_MENU_MAX_W: f32 = 520.0;

/// The Set Priority submenu (UI-PRC-020): the autogroup note, Realtime, the five nice classes,
/// Idle, a separator and Custom…. The current class has a check mark; with several targets only
/// when every one shares it (PROC-066, PROC-069). The scheduler policy comes from the latest
/// snapshot's extras, as in the Priority column.
pub fn priority_submenu(
    menu: TlMenu,
    targets: &CapturedTargets,
    _window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let snapshot = cx.try_global::<EngineGlobal>().map(|e| e.0.latest());
    let current = snapshot
        .as_deref()
        .and_then(|s| current_class(targets, &s.processes));
    // The autogroup note is one long line, wider than the default maximum menu width.
    let mut menu = menu
        .max_w(px(PRIORITY_MENU_MAX_W))
        .header(tr(cx, "processes-menu-autogroup-note"));
    for (class, key) in PRIORITY_CLASSES {
        menu = menu.check_item(
            tr(cx, key),
            current == Some(class),
            Box::new(SetPriority { class }),
        );
    }
    menu.separator().item(
        tr(cx, "processes-menu-priority-custom"),
        Box::new(OpenCustomPriority),
    )
}

/// The class every target is in. `None` when a target has no nice value (app, unit and pseudo
/// rows), when its policy is not known, when they differ, or when a target is in no class.
fn current_class(t: &CapturedTargets, p: &ProcessSnapshot) -> Option<PriorityClass> {
    let mut classes = t.items.iter().map(|t| target_class(t, p));
    let first = classes.next()??;
    classes.all(|c| c == Some(first)).then_some(first)
}

/// The class of one process target, read the way the Priority column reads it (PROC-029):
/// FIFO and RR are Realtime, IDLE is Idle, OTHER and BATCH take the class of their nice value.
/// `None` for DEADLINE, for a nice value that is no class, and when the record or its extras
/// are not in `p`.
fn target_class(t: &CapturedTarget, p: &ProcessSnapshot) -> Option<PriorityClass> {
    let nice = t.nice?;
    let NodeId::Process(key) = &t.node else {
        return None;
    };
    let i = p.records.binary_search_by(|r| r.key.cmp(key)).ok()?;
    match p.extras.get(i)?.policy {
        SchedPolicy::Fifo | SchedPolicy::Rr => Some(PriorityClass::Realtime),
        SchedPolicy::Idle => Some(PriorityClass::Idle),
        SchedPolicy::Deadline => None,
        SchedPolicy::Other | SchedPolicy::Batch => PRIORITY_CLASSES
            .iter()
            .map(|(class, _)| *class)
            .find(|class| class.nice() == Some(nice)),
    }
}

/// The Set I/O Priority submenu (UI-PRC-020, PROC-068): the "Scheduler: <name>" note, the five
/// classes, a separator and "Default (follows nice)". Realtime needs a helper session. The
/// current class has a check mark; with several targets only when every one shares it
/// (PROC-069).
pub fn ioprio_submenu(
    menu: TlMenu,
    targets: &CapturedTargets,
    caps: &MenuCaps,
    cx: &mut App,
) -> TlMenu {
    let snapshot = cx.try_global::<EngineGlobal>().map(|e| e.0.latest());
    let helper = snapshot
        .as_ref()
        .map_or(HelperState::Missing, |s| s.health.helper);
    let current = current_io_class(targets);
    let mut menu = menu;
    if let Some(name) = snapshot.as_deref().and_then(system_scheduler) {
        let args = [("name", FluentValue::from(name.to_string()))];
        menu = menu.header(tr_args(cx, "processes-menu-io-scheduler", &args));
    }
    let session = caps.helper_session || helper == HelperState::Unlocked;
    for (class, key) in IO_CLASSES {
        let label = tr(cx, key);
        let checked = current == Some(class);
        menu = if class == IoClass::Realtime && !session {
            let tip = match helper {
                HelperState::Missing => "processes-menu-io-no-helper",
                _ => "processes-menu-io-needs-admin",
            };
            if checked {
                menu.item_disabled_checked(label, Some(tr(cx, tip)))
            } else {
                menu.item_disabled(label, Some(tr(cx, tip)))
            }
        } else {
            menu.check_item(label, checked, Box::new(SetIoPriorityClass { class }))
        };
    }
    menu.separator().check_item(
        tr(cx, "processes-menu-io-default"),
        current == Some(IoClass::Default),
        Box::new(SetIoPriorityClass {
            class: IoClass::Default,
        }),
    )
}

/// The I/O class every target is in. `None` when a target has no I/O priority (app, unit and
/// pseudo rows, or not read yet), when its level is in no class, or when they differ.
fn current_io_class(t: &CapturedTargets) -> Option<IoClass> {
    let mut classes = t
        .items
        .iter()
        .map(|t| t.io_priority.and_then(IoClass::from_model));
    let first = classes.next()??;
    classes.all(|c| c == Some(first)).then_some(first)
}

/// The block scheduler of the system disk (R-207). When two disks are system disks (`/` and
/// `/boot` apart) both must name the same scheduler. `None` when none is known or they differ.
fn system_scheduler(s: &Snapshot) -> Option<Arc<str>> {
    let mut found: Option<Arc<str>> = None;
    let system = s
        .device_details
        .disks
        .iter()
        .map(|(_, d)| d)
        .filter(|d| d.system_disk.validity == Validity::Valid && d.system_disk.value);
    for disk in system {
        let sched = &disk.scheduler;
        if sched.validity != Validity::Valid || sched.value.is_empty() {
            return None;
        }
        match &found {
            Some(name) if *name != sched.value => return None,
            Some(_) => {}
            None => found = Some(Arc::clone(&sched.value)),
        }
    }
    found
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

/// Whether the one app or unit row of `targets` has something to act on: a unit row always,
/// an app row when it has an app.slice cgroup. With no engine there is none.
fn group_has_target(targets: &CapturedTargets, cx: &App) -> bool {
    let [target] = targets.items.as_slice() else {
        return false;
    };
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return false;
    };
    !group_targets(&engine.0.latest(), target).is_empty()
}

/// An app or unit row item that needs a target: disabled with a tooltip when there is none.
fn group_item(
    menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    label: SharedString,
    action: Box<dyn gpui_kit::Action>,
    cx: &App,
) -> TlMenu {
    if !shape.exited && !group_has_target(targets, cx) {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-no-app-cgroup")));
    }
    exitable(menu, shape, label, action, cx)
}

/// Item 2: End Process or End Selected Processes. On an app or unit row it is Kill App (all
/// processes).
fn add_end_process(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    if shape.group {
        let label = tr(cx, "processes-menu-kill-app");
        return group_item(menu, targets, shape, label, Box::new(KillApp), cx);
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

/// Item 19: Process Properties for process rows and App Properties for app and unit rows.
fn add_properties(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    exitable(
        menu,
        shape,
        tr(cx, "processes-menu-properties"),
        Box::new(OpenProperties),
        cx,
    )
}

/// Item 4: Suspend or Resume, by State T. On an app or unit row it is Freeze App, or Thaw App
/// when the row reads as frozen.
fn add_suspend(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    if shape.group {
        let frozen = match (targets.items.as_slice(), cx.try_global::<EngineGlobal>()) {
            ([target], Some(engine)) => group_frozen(&engine.0.latest(), &target.node),
            _ => false,
        };
        return if frozen {
            let label = tr(cx, "processes-menu-thaw-app");
            group_item(menu, targets, shape, label, Box::new(ThawApp), cx)
        } else {
            let label = tr(cx, "processes-menu-freeze-app");
            group_item(menu, targets, shape, label, Box::new(FreezeApp), cx)
        };
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

/// Item 5: Send Signal, built on open from UI-PWN-026. App and unit rows are resolved to
/// primary processes by the action handler.
fn add_send_signal(
    menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let label = tr(cx, "processes-menu-send-signal");
    if shape.exited {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")));
    }
    let all_kernel_threads = !targets.items.is_empty()
        && targets
            .items
            .iter()
            .all(|target| target.kind == RowKind::KernelThread);
    if all_kernel_threads {
        return menu.item_disabled(label, Some(tr(cx, "fnd-action-reject-kernel-thread")));
    }
    menu.submenu(label, window, cx, move |submenu, window, cx| {
        signal_submenu(submenu, all_kernel_threads, window, cx)
    })
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

/// Item 7: Set Affinity…, on one process, app or unit row, or on several rows (E23-005).
fn add_set_affinity(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    if !(shape.process || shape.group || (shape.multi && !shape.pseudo)) {
        return menu;
    }
    let label = tr(cx, "processes-menu-set-affinity");
    exitable(menu, shape, label, Box::new(SetAffinity), cx)
}

/// Item 8: the Set I/O Priority submenu, on process, app and unit rows and on several rows.
/// App and unit rows apply to their current processes (the handler expands them).
fn add_set_io_priority(
    menu: TlMenu,
    targets: &CapturedTargets,
    caps: &MenuCaps,
    shape: Shape,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let label = tr(cx, "processes-menu-set-io-priority");
    if shape.exited {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")));
    }
    let targets = targets.clone();
    let caps = *caps;
    menu.submenu(label, window, cx, move |sub, _window, cx| {
        ioprio_submenu(sub, &targets, &caps, cx)
    })
}

/// Which of the three minigraph items (UI-PRC-013 items 15 to 17) a source is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphKind {
    /// Add Minigraph: the process, or the app on an app or unit row.
    Minigraph,
    /// Add Process Name Graph: every process with this image name.
    Name,
    /// Add App Name Graph: every process of the app or unit.
    App,
}

/// The graph columns of the three submenus, in order, with their label keys (PERF-087).
const GRAPH_COLUMNS: [(GraphColumn, &str); 6] = [
    (GraphColumn::Cpu, "processes-menu-metric-cpu"),
    (GraphColumn::Memory, "processes-menu-metric-memory"),
    (GraphColumn::Disk, "processes-menu-metric-disk"),
    (GraphColumn::Network, "processes-menu-metric-network"),
    (GraphColumn::Gpu, "processes-menu-metric-gpu"),
    (GraphColumn::Vram, "processes-menu-metric-vram"),
];

/// The card source `kind` creates for one captured row, `None` when the item is not offered on
/// it or the row's identity is not in `snapshot`. A process row gives its process, image name
/// and app id. A unit row gives `App(<unit name>)` for items 15 and 17. An app row gives
/// `App(<app id>)` for the same two. Name has no source on an app or unit row.
pub fn graph_source(
    kind: GraphKind,
    target: &CapturedTarget,
    snapshot: &Snapshot,
) -> Option<MinigraphSource> {
    match (&target.node, target.kind) {
        (NodeId::Process(key), RowKind::Process | RowKind::KernelThread) => {
            let meta = || {
                snapshot
                    .processes
                    .records
                    .iter()
                    .find(|r| r.key == *key)
                    .map(|r| &r.meta)
            };
            match kind {
                GraphKind::Minigraph => Some(MinigraphSource::Process { key: *key }),
                GraphKind::Name => meta().map(|m| MinigraphSource::Name(m.name.clone())),
                GraphKind::App => meta()?.app_id.clone().map(MinigraphSource::App),
            }
        }
        _ if kind == GraphKind::Name => None,
        (NodeId::Unit(unit), RowKind::Unit) => Some(MinigraphSource::App(unit.name.clone())),
        (node, RowKind::App) => {
            let group = snapshot.table.groups.iter().find(|g| &g.node == node)?;
            match &group.class {
                GroupKind::App { app_id } if group.kind == GroupRowKind::App => {
                    Some(MinigraphSource::App(app_id.clone()))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Items 15 to 17: Add Minigraph >, Add Process Name Graph > and Add App Name Graph >, on one
/// process, app or unit row. Each lists the six columns; one whose card exists is checked and
/// disabled with "Already added". An exited row disables the three items.
fn add_graph_menus(
    mut menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    let [target] = targets.items.as_slice() else {
        return menu;
    };
    if shape.multi || shape.pseudo {
        return menu;
    }
    let Some(engine) = cx.try_global::<EngineGlobal>() else {
        return menu;
    };
    let snapshot = engine.0.latest();
    let items = [
        (GraphKind::Minigraph, "processes-menu-add-minigraph"),
        (GraphKind::Name, "processes-menu-add-name-graph"),
        (GraphKind::App, "processes-menu-add-app-graph"),
    ];
    for (kind, key) in items {
        let Some(source) = graph_source(kind, target, &snapshot) else {
            continue;
        };
        let label = tr(cx, key);
        if shape.exited {
            menu = menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")));
            continue;
        }
        menu = menu.submenu(label, window, cx, move |sub, _window, cx| {
            let panel = MinigraphPanel::global(cx);
            GRAPH_COLUMNS.iter().fold(sub, |sub, (column, key)| {
                let column = *column;
                let label = tr(cx, key);
                let added = panel
                    .as_ref()
                    .is_some_and(|p| p.read(cx).has_source(&source, column));
                if added {
                    return sub.item_disabled_checked(
                        label,
                        Some(tr(cx, "processes-menu-already-added")),
                    );
                }
                let action: Box<dyn gpui_kit::Action> = match kind {
                    GraphKind::Minigraph => Box::new(AddMinigraph { column }),
                    GraphKind::Name => Box::new(AddNameGraph { column }),
                    GraphKind::App => Box::new(AddAppGraph { column }),
                };
                sub.item(label, action)
            })
        });
    }
    menu
}

/// The tray graph columns of the Add submenu, in order, with their label keys.
const TRAY_COLUMNS: [(&str, &str); 6] = [
    ("cpu", "processes-menu-metric-cpu"),
    ("memory", "processes-menu-metric-memory"),
    ("disk", "processes-menu-metric-disk"),
    ("network", "processes-menu-metric-network"),
    ("gpu", "processes-menu-metric-gpu"),
    ("vram", "processes-menu-metric-vram"),
];

/// The tray source string of the one captured target (R-100): a process row, an app row or a
/// unit row. `None` for anything else, which leaves the item out.
fn tray_source(targets: &CapturedTargets, cx: &App) -> Option<SharedString> {
    let [target] = targets.items.as_slice() else {
        return None;
    };
    let source = match (&target.node, target.kind) {
        (NodeId::Process(key), RowKind::Process | RowKind::KernelThread) => {
            MinigraphSource::Process { key: *key }
        }
        (NodeId::Unit(unit), RowKind::Unit) => MinigraphSource::App(unit.name.clone()),
        (node, RowKind::App) => {
            let engine = cx.try_global::<EngineGlobal>()?;
            let snapshot = engine.0.latest();
            let group = snapshot.table.groups.iter().find(|g| &g.node == node)?;
            match &group.class {
                GroupKind::App { app_id } if group.kind == GroupRowKind::App => {
                    MinigraphSource::App(app_id.clone())
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(source.to_state_string().into())
}

/// Item 18: Add tray icon graph >, or Remove tray icon graph when one exists for this source
/// (UI-PRC-013, PROC-094, PERF-088). One process, app or unit row only. Add has no "Already
/// added" state: any graph with the source turns the item into Remove.
fn add_tray_graph(
    menu: TlMenu,
    targets: &CapturedTargets,
    shape: Shape,
    window: &mut Window,
    cx: &mut App,
) -> TlMenu {
    if shape.multi || shape.pseudo {
        return menu;
    }
    let Some(source) = tray_source(targets, cx) else {
        return menu;
    };
    let available = tray_available(cx);
    let existing = if available {
        TrayBridge::global(cx)
            .read(cx)
            .graphs()
            .iter()
            .find(|g| g.source == source)
            .map(|g| g.column.clone())
    } else {
        None
    };
    let label = tr(
        cx,
        if existing.is_some() {
            "processes-menu-remove-tray-graph"
        } else {
            "processes-menu-add-tray-graph"
        },
    );
    if shape.exited {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-process-exited")));
    }
    if !available {
        return menu.item_disabled(label, Some(tr(cx, "perf-tip-no-tray")));
    }
    if let Some(column) = existing {
        return menu.item(label, Box::new(RemoveTrayGraph { source, column }));
    }
    menu.submenu(label, window, cx, move |sub, _window, cx| {
        TRAY_COLUMNS.iter().fold(sub, |sub, (column, key)| {
            sub.item(
                tr(cx, key),
                Box::new(AddTrayGraph {
                    source: source.clone(),
                    column: (*column).into(),
                }),
            )
        })
    })
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

/// Item 9: Show Window, on one process, kernel-thread, app or unit row (PROC-054). Hidden
/// without the KWin bridge. Disabled with "No window" when the row owns no window.
fn add_show_window(
    menu: TlMenu,
    targets: &CapturedTargets,
    caps: &MenuCaps,
    shape: Shape,
    cx: &App,
) -> TlMenu {
    let [target] = targets.items.as_slice() else {
        return menu;
    };
    if !(shape.process || shape.group) || !caps.kwin_bridge {
        return menu;
    }
    let label = tr(cx, "processes-menu-show-window");
    if !shape.exited {
        let has_window = cx
            .try_global::<EngineGlobal>()
            .is_some_and(|g| topmost_window(&g.0.latest(), target).is_some());
        if !has_window {
            return menu.item_disabled(label, Some(tr(cx, "processes-menu-no-window")));
        }
    }
    exitable(menu, shape, label, Box::new(ShowWindow), cx)
}

/// Item 10: Restart Plasma Shell on the plasmashell row, else Restart unit on a unit row or a
/// unit's MainPID row (PROC-072). Never offered for the compositor.
fn add_restart_unit(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    let [target] = targets.items.as_slice() else {
        return menu;
    };
    if !(shape.process || (shape.group && target.kind == RowKind::Unit)) {
        return menu;
    }
    if let Some(unit) = plasma_restart_unit(target) {
        let action = RestartUnit { unit };
        let label = tr(cx, "processes-menu-restart-plasma");
        return exitable(menu, shape, label, Box::new(action), cx);
    }
    let Some(unit) = &target.unit else {
        return menu;
    };
    if is_compositor(&target.name, &unit.name) {
        return menu;
    }
    let label = tr_args(
        cx,
        "processes-menu-restart-unit",
        &[("unit", unit.name.as_ref().into())],
    );
    let action = RestartUnit { unit: unit.clone() };
    exitable(menu, shape, label, Box::new(action), cx)
}

/// The compositor must never be restarted from here (PROC-072).
fn is_compositor(name: &str, unit: &str) -> bool {
    matches!(name, "kwin_wayland" | "gnome-shell")
        || unit.starts_with("plasma-kwin_wayland")
        || unit.starts_with("org.gnome.Shell@")
}

/// Item 20: File properties, on one process row. Hidden without a FileManager1 owner.
/// Disabled when the executable was deleted.
fn add_file_properties(
    menu: TlMenu,
    targets: &CapturedTargets,
    caps: &MenuCaps,
    shape: Shape,
    cx: &App,
) -> TlMenu {
    if !shape.process || !caps.file_manager1 {
        return menu;
    }
    let label = tr(cx, "processes-menu-file-properties");
    if !shape.exited && targets.items.iter().any(|t| t.exe_deleted) {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-executable-deleted")));
    }
    exitable(menu, shape, label, Box::new(FileProperties), cx)
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

/// Item 22: Open in VirusTotal, on one process row (PROC-074).
fn add_open_virustotal(menu: TlMenu, shape: Shape, cx: &App) -> TlMenu {
    if !shape.process {
        return menu;
    }
    exitable(
        menu,
        shape,
        tr(cx, "processes-menu-open-virustotal"),
        Box::new(OpenVirusTotal),
        cx,
    )
}

/// Item 23: Create dump file, on one process row (PROC-075). Disabled without gdb (gcore). An
/// exited row shows the exited reason first.
fn add_create_dump(menu: TlMenu, caps: &MenuCaps, shape: Shape, cx: &App) -> TlMenu {
    if !shape.process {
        return menu;
    }
    let label = tr(cx, "processes-menu-create-dump");
    if !shape.exited && !caps.gdb {
        return menu.item_disabled(label, Some(tr(cx, "processes-menu-install-gdb")));
    }
    exitable(menu, shape, label, Box::new(CreateDump), cx)
}

/// Item 24: Go to parent process, on one process row whose parent PID is not 0.
fn add_go_to_parent(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    if !shape.process || targets.items.iter().all(|t| t.ppid == 0) {
        return menu;
    }
    let label = tr(cx, "processes-menu-go-to-parent");
    exitable(menu, shape, label, Box::new(GoToParent), cx)
}

/// Item 25: Go to service, on one unit row or one process row that is a unit's MainPID.
fn add_go_to_service(menu: TlMenu, targets: &CapturedTargets, shape: Shape, cx: &App) -> TlMenu {
    let [target] = targets.items.as_slice() else {
        return menu;
    };
    let Some(unit) = &target.unit else {
        return menu;
    };
    if !matches!(
        target.kind,
        RowKind::Unit | RowKind::Process | RowKind::KernelThread
    ) {
        return menu;
    }
    let manager = match unit.manager {
        Manager::System => ManagerArg::System,
        Manager::User => ManagerArg::User,
    };
    let action = GoToService {
        manager,
        name: SharedString::from(&*unit.name),
        origin: JumpOrigin::Processes,
    };
    let label = tr(cx, "processes-menu-go-to-service");
    exitable(menu, shape, label, Box::new(action), cx)
}
