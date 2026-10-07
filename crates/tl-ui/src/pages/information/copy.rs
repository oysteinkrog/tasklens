//! Copy text of the Information tab (UI-SYS-083, R-012, D-007).
//!
//! A section is its title line, then one "Label: value" line per row. A value that is not Valid
//! or Stale copies its PERF-013 word, several lines join with ", ", and a Missing value (not
//! drawn) is left out. One blank line goes between sub-blocks and one between sections, with
//! none at the end. Copies always use the page's full rows, whatever the filter. Memory module
//! serials are never built into a row (D-007), so they cannot reach the copy.

use std::sync::Arc;

use tl_model::validity::Validity;

use super::section::{InfoRow, InfoSectionId, value_copy_text};

/// The capacity one copy command reserves.
pub const COPY_CAPACITY: usize = 8 * 1024;

/// Appends section `id` as copy text.
pub fn section_text(id: InfoSectionId, rows: &[InfoRow], out: &mut String) {
    out.push_str(&id.title());
    let mut block = None;
    for row in rows
        .iter()
        .filter(|r| r.value.validity != Validity::Missing)
    {
        if block.is_some_and(|b| b != row.block) {
            out.push('\n');
        }
        block = Some(row.block);
        out.push('\n');
        out.push_str(&row.label);
        out.push_str(": ");
        value_copy_text(row, out);
    }
}

/// Appends every section in [`InfoSectionId::ALL`] order, one blank line apart.
pub fn all_text(sections: &[(InfoSectionId, Arc<[InfoRow]>); 10], out: &mut String) {
    for (n, (id, rows)) in sections.iter().enumerate() {
        if n > 0 {
            out.push_str("\n\n");
        }
        section_text(*id, rows, out);
    }
}
