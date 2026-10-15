//! The Processes tab's text in the shell's status chip (UI-PRC-018, UI-SHL-050): the process
//! count, the selection size and the restricted count. The paused tag is the shell's (E20-012).

use fluent_bundle::FluentValue;
use gpui_kit::App;
use smallvec::SmallVec;
use tl_model::health::HelperState;
use tl_model::table::ProcessTableView;

use crate::actions::{EndHelperSession, StartHelperSession};
use crate::i18n::{tr, tr_args};
use crate::window::status::{StatusAction, StatusActionKind};

/// What the chip text shows. The text is rebuilt only when this changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusCounts {
    /// Processes after Hide system processes, before the filter. Pseudo rows are not processes.
    pub total: u32,
    /// Processes left after the filter.
    pub shown: u32,
    /// The filter text is set.
    pub filtered: bool,
    /// Selected rows present in the view; rows the filter hides are not counted.
    pub selected: u32,
    /// Processes with at least one restricted value.
    pub restricted: u32,
    /// The helper session state. Missing keeps the restricted count plain text, because the
    /// shield is hidden and there is no way to start a session.
    pub helper: HelperState,
}

impl StatusCounts {
    /// The counts of an accepted view (E03-A2).
    pub fn of(view: &ProcessTableView, filtered: bool, selected: usize) -> Self {
        Self {
            total: view.total,
            shown: view.shown,
            filtered,
            selected: u32::try_from(selected).unwrap_or(u32::MAX),
            restricted: view.restricted,
            helper: HelperState::Missing,
        }
    }
}

/// Writes the chip text for `c`: "296 Processes" or "41 / 296 Processes", then
/// " | 3 processes selected" for two or more selected rows, then " | 42 restricted" while the
/// helper is missing. With the helper installed the restricted count is a [`write_actions`] link.
pub fn write_status(out: &mut String, c: StatusCounts, cx: &App) {
    out.clear();
    let text = if c.filtered {
        tr_args(
            cx,
            "processes-status-filtered",
            &[
                ("shown", FluentValue::from(c.shown)),
                ("total", FluentValue::from(c.total)),
            ],
        )
    } else {
        tr_args(
            cx,
            "processes-status-count",
            &[("total", FluentValue::from(c.total))],
        )
    };
    out.push_str(&text);
    if c.selected >= 2 {
        out.push_str(" | ");
        out.push_str(&tr_args(
            cx,
            "processes-status-selected",
            &[("count", FluentValue::from(c.selected))],
        ));
    }
    if c.restricted > 0 && c.helper == HelperState::Missing {
        out.push_str(" | ");
        out.push_str(&tr_args(
            cx,
            "processes-status-restricted",
            &[("count", FluentValue::from(c.restricted))],
        ));
    }
}

/// Writes the clickable chip parts for `c` (PROC-051, PROC-076): the restricted count as a link
/// that starts the helper session, then the "Administrator mode" badge while it is active.
pub fn write_actions(out: &mut SmallVec<[StatusAction; 2]>, c: StatusCounts, cx: &App) {
    out.clear();
    if c.restricted > 0 && c.helper != HelperState::Missing {
        out.push(StatusAction {
            kind: StatusActionKind::Link,
            label: tr_args(
                cx,
                "processes-status-restricted",
                &[("count", FluentValue::from(c.restricted))],
            ),
            tooltip: tr(cx, "processes-status-restricted-tooltip"),
            action: Box::new(StartHelperSession),
        });
    }
    if c.helper == HelperState::Unlocked {
        out.push(StatusAction {
            kind: StatusActionKind::Badge,
            label: tr(cx, "processes-status-administrator-mode"),
            tooltip: tr(cx, "fnd-shield-unlocked"),
            action: Box::new(EndHelperSession),
        });
    }
}
