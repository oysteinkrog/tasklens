//! Processes page Actions and the captured-target types the row menu, the keymap and the
//! handlers share (E22-002, UI-PRC-009, UI-PRC-013, UI-PRC-020, PROC-001, R-128, ADR-005,
//! ADR-009).
//!
//! One command is one Action in the `tasklens` namespace: the key binding, the menu item and
//! the toolbar button dispatch the same one. Shared table Actions (`OpenProperties`,
//! `CopyCell`, `SelectAll` and the others) live in `table/actions.rs`; window-level Actions
//! (`FocusFilter`, `ToggleHideSystem`, `SetViewMode` and the others) live in `actions.rs`.

use gpui_kit::gpui::private::schemars::JsonSchema;
use gpui_kit::{Action, SharedString, gpui};
use serde::Deserialize;
use smallvec::SmallVec;
use tl_model::actions::IoPriority;
use tl_model::ids::{ColumnId, NodeId, ProcessKey};
use tl_model::process::ProcFlags;
use tl_model::table::UnitRef;

use crate::table::tree_table::RowKind;

gpui_kit::actions!(
    tasklens,
    [
        EndTask,
        EndProcess,
        EndProcessTree,
        KillApp,
        SuspendProcess,
        ResumeProcess,
        FreezeApp,
        ThawApp,
        ShowWindow,
        CopyAllMetrics,
        FileProperties,
        OpenFileLocation,
        OpenVirusTotal,
        CreateDump,
        GoToParent,
        ProcessesShowHidden,
    ]
);

/// The priority classes of the Set Priority submenu (PROC-068).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum PriorityClass {
    Realtime,
    High,
    AboveNormal,
    Normal,
    BelowNormal,
    Low,
    Idle,
}

impl PriorityClass {
    /// The nice value of the class. `Realtime` and `Idle` are scheduling policies, not nice
    /// values, and return `None` (they map to `SetSched`, E22-036).
    pub fn nice(self) -> Option<i8> {
        match self {
            Self::High => Some(-10),
            Self::AboveNormal => Some(-5),
            Self::Normal => Some(0),
            Self::BelowNormal => Some(5),
            Self::Low => Some(19),
            Self::Realtime | Self::Idle => None,
        }
    }
}

/// The I/O priority classes of the Set I/O Priority submenu (PROC-068, R-093).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[schemars(crate = "gpui::private::schemars")]
pub enum IoClass {
    Realtime,
    High,
    Normal,
    Low,
    Idle,
    Default,
}

impl IoClass {
    pub fn to_model(self) -> IoPriority {
        match self {
            Self::Realtime => IoPriority::Realtime(0),
            Self::High => IoPriority::BestEffort(0),
            Self::Normal => IoPriority::BestEffort(4),
            Self::Low => IoPriority::BestEffort(7),
            Self::Idle => IoPriority::Idle,
            Self::Default => IoPriority::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetPriority {
    pub class: PriorityClass,
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct SetIoPriorityClass {
    pub class: IoClass,
}

/// Restarts a systemd unit. `no_json`: `UnitRef` holds an `Arc<str>` and has no serde derive
/// (ADR-002), so this Action is dispatched from the menu and never read from a keymap file.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = tasklens, no_json)]
pub struct RestartUnit {
    pub unit: UnitRef,
}

/// Brings a window to the front. `internal_id` is the KWin window id.
#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = tasklens)]
#[schemars(crate = "gpui::private::schemars")]
pub struct ActivateWindow {
    pub internal_id: SharedString,
}

/// The most targets one action takes (PROC-001).
pub const MAX_TARGETS: usize = 256;

/// One row an action works on, captured when the menu opens or the key is pressed (PROC-001).
/// Holds no command line or environment.
#[derive(Clone, Debug)]
pub struct CapturedTarget {
    /// Identity, never an index.
    pub node: NodeId,
    /// App, Unit, Process, KernelThread or Pseudo; never GroupHeader.
    pub kind: RowKind,
    /// The display name as drawn in the Name cell, without the " (N)" count.
    pub name: SharedString,
    /// `None` for app, unit and pseudo rows.
    pub pid: Option<u32>,
    pub uid: u32,
    pub user: SharedString,
    pub flags: ProcFlags,
    /// Processes of an app or unit row, 1 for a process row.
    pub member_count: u32,
}

#[derive(Clone, Debug, Default)]
pub struct CapturedTargets {
    /// View order, at most `MAX_TARGETS`.
    pub items: SmallVec<[CapturedTarget; 1]>,
    /// The column under the pointer at right-click; `None` from the keyboard.
    pub clicked_col: Option<ColumnId>,
    /// True when the selection had more than `MAX_TARGETS` rows.
    pub truncated: bool,
}

impl CapturedTargets {
    /// Keeps the first `MAX_TARGETS` targets in view order and sets `truncated` when there
    /// were more.
    pub fn new(
        targets: impl IntoIterator<Item = CapturedTarget>,
        clicked_col: Option<ColumnId>,
    ) -> Self {
        let mut iter = targets.into_iter();
        let items: SmallVec<[CapturedTarget; 1]> = iter.by_ref().take(MAX_TARGETS).collect();
        let truncated = iter.next().is_some();
        Self {
            items,
            clicked_col,
            truncated,
        }
    }

    pub fn is_multi(&self) -> bool {
        self.items.len() >= 2
    }

    pub fn any_exited(&self) -> bool {
        self.items
            .iter()
            .any(|t| t.flags.contains(ProcFlags::EXITED))
    }

    /// The process keys of the Process and KernelThread items, in view order.
    pub fn process_keys(&self) -> SmallVec<[ProcessKey; 1]> {
        self.items
            .iter()
            .filter(|t| matches!(t.kind, RowKind::Process | RowKind::KernelThread))
            .filter_map(|t| match t.node {
                NodeId::Process(key) => Some(key),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tl_model::table::Manager;
    use tl_model::time::BootId;

    #[test]
    fn priority_class_nice_values() {
        assert_eq!(PriorityClass::High.nice(), Some(-10));
        assert_eq!(PriorityClass::AboveNormal.nice(), Some(-5));
        assert_eq!(PriorityClass::Normal.nice(), Some(0));
        assert_eq!(PriorityClass::BelowNormal.nice(), Some(5));
        assert_eq!(PriorityClass::Low.nice(), Some(19));
        assert_eq!(PriorityClass::Realtime.nice(), None);
        assert_eq!(PriorityClass::Idle.nice(), None);
    }

    #[test]
    fn io_class_maps_to_model() {
        let normal = SetIoPriorityClass {
            class: IoClass::Normal,
        };
        assert_eq!(normal.class.to_model(), IoPriority::BestEffort(4));
        assert_eq!(IoClass::Realtime.to_model(), IoPriority::Realtime(0));
        assert_eq!(IoClass::High.to_model(), IoPriority::BestEffort(0));
        assert_eq!(IoClass::Low.to_model(), IoPriority::BestEffort(7));
        assert_eq!(IoClass::Idle.to_model(), IoPriority::Idle);
        assert_eq!(IoClass::Default.to_model(), IoPriority::None);
    }

    fn page_names() -> Vec<&'static str> {
        vec![
            EndTask.name(),
            EndProcess.name(),
            EndProcessTree.name(),
            KillApp.name(),
            SuspendProcess.name(),
            ResumeProcess.name(),
            FreezeApp.name(),
            ThawApp.name(),
            ShowWindow.name(),
            CopyAllMetrics.name(),
            FileProperties.name(),
            OpenFileLocation.name(),
            OpenVirusTotal.name(),
            CreateDump.name(),
            GoToParent.name(),
            ProcessesShowHidden.name(),
            SetPriority::name_for_type(),
            SetIoPriorityClass::name_for_type(),
            RestartUnit::name_for_type(),
            ActivateWindow::name_for_type(),
        ]
    }

    #[test]
    fn action_names_are_unique() {
        let names = page_names();
        let mut seen = HashSet::new();
        for name in &names {
            assert!(name.starts_with("tasklens::"), "{name}");
            assert!(seen.insert(*name), "declared twice: {name}");
        }
        assert_eq!(names.len(), 20);

        // No name here is also declared by the table or the shell.
        use crate::actions as shell;
        use crate::table::actions as table;
        let others = [
            table::SelectAll.name(),
            table::ToggleExpand.name(),
            table::OpenProperties.name(),
            table::CopyRow.name(),
            table::CopyRowsAsTable.name(),
            table::CollapseAll.name(),
            table::OpenContextMenu.name(),
            table::CopyCell::name_for_type(),
            table::MoveFocus::name_for_type(),
            shell::FocusFilter.name(),
            shell::ClearFilter.name(),
            shell::ToggleHideSystem.name(),
            shell::CycleViewMode.name(),
            shell::SetViewMode::name_for_type(),
            shell::ToggleSetting::name_for_type(),
            shell::StartHelperSession.name(),
        ];
        for other in others {
            assert!(!seen.contains(other), "declared twice: {other}");
        }
    }

    fn target(pid: u32, flags: ProcFlags) -> CapturedTarget {
        CapturedTarget {
            node: NodeId::Process(ProcessKey {
                start_ticks: u64::from(pid),
                boot_id: BootId(1),
                pid,
            }),
            kind: RowKind::Process,
            name: "p".into(),
            pid: Some(pid),
            uid: 1000,
            user: "u".into(),
            flags,
            member_count: 1,
        }
    }

    #[test]
    fn capture_caps_at_max_targets() {
        let t = CapturedTargets::new((0..300).map(|p| target(p, ProcFlags::empty())), None);
        assert_eq!(t.items.len(), MAX_TARGETS);
        assert!(t.truncated);
        assert!(t.is_multi());
        assert_eq!(t.items[255].pid, Some(255));

        let t = CapturedTargets::new((0..256).map(|p| target(p, ProcFlags::empty())), None);
        assert_eq!(t.items.len(), MAX_TARGETS);
        assert!(!t.truncated);
    }

    #[test]
    fn targets_helpers() {
        let unit = CapturedTarget {
            node: NodeId::Unit(UnitRef {
                manager: Manager::System,
                name: Arc::from("foo.service"),
            }),
            kind: RowKind::Unit,
            pid: None,
            member_count: 3,
            ..target(0, ProcFlags::empty())
        };
        let one = CapturedTargets::new([target(7, ProcFlags::empty())], Some(ColumnId::Name));
        assert!(!one.is_multi());
        assert!(!one.any_exited());
        assert!(!one.items.spilled());

        let mixed = CapturedTargets::new(
            [
                unit,
                target(7, ProcFlags::EXITED),
                target(9, ProcFlags::NEW),
            ],
            None,
        );
        assert!(mixed.is_multi());
        assert!(mixed.any_exited());
        let pids: Vec<u32> = mixed.process_keys().iter().map(|k| k.pid).collect();
        assert_eq!(pids, [7, 9]);
    }
}
