//! The Processes tab's text in the shell's status chip (UI-PRC-018, UI-SHL-050): the process
//! count, the selection size and the restricted count. The paused tag is the shell's (E20-012).

use fluent_bundle::FluentValue;
use gpui_kit::App;
use tl_model::table::ProcessTableView;

use crate::i18n::tr_args;

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
        }
    }
}

/// Writes the chip text for `c`: "296 Processes" or "41 / 296 Processes", then
/// " | 3 processes selected" for two or more selected rows, then " | 42 restricted".
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
    if c.restricted > 0 {
        out.push_str(" | ");
        out.push_str(&tr_args(
            cx,
            "processes-status-restricted",
            &[("count", FluentValue::from(c.restricted))],
        ));
    }
}
