//! The Information filter (UI-SYS-084, PERF-094, APP-028).
//!
//! The page filters its rows in the UI, with no engine query. For one section and a lowercased
//! needle, in this order:
//! 1. An empty needle keeps every row.
//! 2. A section title that contains the needle keeps every row.
//! 3. Otherwise a row stays when its label or its shown value (the text, or the PERF-013 word of
//!    a value that is not Valid or Stale, every line) contains the needle.
//! 4. A sub-block with a row that stays also keeps its first row ("Module", "Model", "Name",
//!    "Connection"), so the block keeps its identity.
//! 5. A section with no row left is hidden, band included.
//!
//! Matching ignores case and allocates nothing per row. Nothing is highlighted.

use std::sync::Arc;

use smallvec::SmallVec;

use super::section::InfoRow;
use super::sections::BLOCK_FIRST_LABELS;
use crate::components::kv_grid::{KvWords, shown_text_in};

/// The rows of one section that stay visible for `needle`, which is already lowercased. An
/// empty result hides the section.
pub fn filter_section(title: &str, rows: &[InfoRow], needle: &str) -> Arc<[InfoRow]> {
    if needle.is_empty() || contains_ci(title, needle) {
        return Arc::from(rows);
    }
    thread_local! {
        static WORDS: KvWords = KvWords::default();
    }
    let mut keep: SmallVec<[bool; 64]> = WORDS.with(|words| {
        rows.iter()
            .map(|row| {
                contains_ci(&row.label, needle)
                    || shown_text_in(&row.value, words).is_some_and(|t| contains_ci(&t, needle))
            })
            .collect()
    });
    // Rule 4: a block with a kept row keeps its first row.
    let (mut block, mut head) = (0u8, None);
    for (i, row) in rows.iter().enumerate() {
        if row.block != block {
            block = row.block;
            head = (block != 0 && BLOCK_FIRST_LABELS.contains(&row.label.as_ref())).then_some(i);
        }
        if keep[i]
            && let Some(at) = head
        {
            keep[at] = true;
        }
    }
    rows.iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(row, _)| row.clone())
        .collect()
}

/// Whether `hay` contains `needle` (already lowercased), ignoring case.
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.char_indices().any(|(at, _)| {
        let mut rest = hay[at..].chars().flat_map(char::to_lowercase);
        needle.chars().all(|n| rest.next() == Some(n))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_ignores_case() {
        assert!(contains_ci("Kernel version", "kernel"));
        assert!(contains_ci("Linux KERNEL", "kernel"));
        assert!(contains_ci("anything", ""));
        assert!(!contains_ci("Kern", "kernel"));
        assert!(contains_ci("Ünïcode", "ünï"));
    }
}
