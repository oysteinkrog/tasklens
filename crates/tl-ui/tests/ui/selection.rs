//! Selection by identity (E18-001, UI-PRC-008, ADR-009).

use rustc_hash::{FxHashMap, FxHashSet};
use tl_ui::table::selection::{Selection, SelectionView};

/// Rows in display order; ids in `headers` are group headers.
struct VecView {
    rows: Vec<u32>,
    headers: FxHashSet<u32>,
    index: FxHashMap<u32, usize>,
}

impl VecView {
    fn new(rows: &[u32]) -> Self {
        Self::with_headers(rows, &[])
    }

    fn with_headers(rows: &[u32], headers: &[u32]) -> Self {
        Self {
            rows: rows.to_vec(),
            headers: headers.iter().copied().collect(),
            index: rows.iter().enumerate().map(|(ix, &id)| (id, ix)).collect(),
        }
    }
}

impl SelectionView<u32> for VecView {
    fn len(&self) -> usize {
        self.rows.len()
    }
    fn id(&self, ix: usize) -> u32 {
        self.rows[ix]
    }
    fn index_of(&self, id: u32) -> Option<usize> {
        self.index.get(&id).copied()
    }
    fn selectable(&self, ix: usize) -> bool {
        !self.headers.contains(&self.rows[ix])
    }
}

const A: u32 = 1;
const B: u32 = 2;
const C: u32 = 3;
const D: u32 = 4;

fn sorted(s: &Selection<u32>) -> Vec<u32> {
    let mut ids: Vec<u32> = s.ids().collect();
    ids.sort_unstable();
    ids
}

#[test]
fn selection_click_ctrl_shift_matrix() {
    let v = VecView::new(&[A, B, C, D]);

    // Click B, then Shift+click D.
    let mut s = Selection::new();
    s.click(B);
    assert_eq!(sorted(&s), [B]);
    assert_eq!((s.anchor(), s.focus()), (Some(B), Some(B)));
    s.shift_click(D, &v);
    assert_eq!(sorted(&s), [B, C, D]);
    assert_eq!((s.anchor(), s.focus()), (Some(B), Some(D)));
    // Shift+click upward replaces the range, anchor stays.
    s.shift_click(A, &v);
    assert_eq!(sorted(&s), [A, B]);
    assert_eq!((s.anchor(), s.focus()), (Some(B), Some(A)));

    // Shift+click with no anchor behaves as Click.
    let mut s = Selection::new();
    s.shift_click(C, &v);
    assert_eq!(sorted(&s), [C]);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(C)));

    // Ctrl+click adds: focus and anchor move to the added row.
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(D);
    assert_eq!(sorted(&s), [B, D]);
    assert_eq!((s.anchor(), s.focus()), (Some(D), Some(D)));

    // Ctrl+click removes: focus moves, anchor stays.
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(D);
    s.ctrl_click(B);
    s.ctrl_click(B);
    // Set {B, D}, anchor B after re-adding B.
    assert_eq!(sorted(&s), [B, D]);
    assert_eq!(s.anchor(), Some(B));
    s.ctrl_click(D);
    assert_eq!(sorted(&s), [B]);
    assert_eq!((s.anchor(), s.focus()), (Some(B), Some(D)));

    // Ctrl+Shift+click adds the anchor..row range.
    let mut s = Selection::new();
    s.click(A);
    s.ctrl_shift_click(C, &v);
    assert_eq!(sorted(&s), [A, B, C]);
    assert_eq!((s.anchor(), s.focus()), (Some(A), Some(C)));
    // It keeps rows outside the range.
    let mut s = Selection::new();
    s.click(D);
    s.ctrl_click(A);
    s.ctrl_shift_click(B, &v);
    assert_eq!(sorted(&s), [A, B, D]);

    // Right-click.
    let mut s = Selection::new();
    s.click(B);
    assert!(!s.right_click(B));
    assert_eq!(sorted(&s), [B]);
    assert!(s.right_click(C));
    assert_eq!(sorted(&s), [C]);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(C)));

    // Up/Down: set = {focus}, anchor = focus. Shift extends from the anchor.
    let mut s = Selection::new();
    s.click(B);
    s.move_focus(1, false, &v);
    assert_eq!(sorted(&s), [C]);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(C)));
    s.move_focus(1, true, &v);
    assert_eq!(sorted(&s), [C, D]);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(D)));
    s.move_focus(-3, true, &v);
    assert_eq!(sorted(&s), [A, B, C]);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(A)));
    // Clamped at the top.
    s.move_focus(-1, false, &v);
    assert_eq!(sorted(&s), [A]);

    // No focus: the first Down or Up selects the first selectable row (UI-PRC-009).
    let mut s = Selection::new();
    s.move_focus(1, false, &v);
    assert_eq!(sorted(&s), [A]);
    assert_eq!((s.anchor(), s.focus()), (Some(A), Some(A)));
    let mut s = Selection::new();
    s.move_focus(-1, false, &v);
    assert_eq!(s.focus(), Some(A));

    // Ctrl+Space toggles the focused row.
    let mut s = Selection::new();
    s.click(B);
    s.set_focus_only(D);
    assert_eq!(sorted(&s), [B]);
    assert_eq!(s.anchor(), Some(B));
    s.toggle_focus();
    assert_eq!(sorted(&s), [B, D]);
    s.toggle_focus();
    assert_eq!(sorted(&s), [B]);

    // Clear.
    s.clear();
    assert_eq!(s.len(), 0);
    assert_eq!((s.anchor(), s.focus()), (None, None));
}

#[test]
fn selection_survives_resort() {
    let v = VecView::new(&[A, B, C, D]);
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(C);
    s.ctrl_click(B);
    s.ctrl_click(B);

    let resorted = VecView::new(&[D, C, B, A]);
    assert!(s.contains(&B));
    assert_eq!(resorted.index_of(B), Some(2));
    assert_eq!(s.anchor(), Some(B));
    assert_eq!(s.focus(), Some(B));
    assert_eq!(s.in_view_order(&resorted).as_slice(), [C, B]);
    assert_eq!(s.in_view_order(&v).as_slice(), [B, C]);

    // Gestures after the re-sort use the new order: Shift+click A from anchor B.
    s.shift_click(A, &resorted);
    assert_eq!(sorted(&s), [A, B]);
    s.shift_click(D, &resorted);
    assert_eq!(sorted(&s), [B, C, D]);
}

#[test]
fn selection_skips_group_headers() {
    // Row 0 is a header, rows 1..3 processes, row 2 also a header in the second view.
    let h0: u32 = 100;
    let v = VecView::with_headers(&[h0, A, B, C], &[h0]);
    let mut s = Selection::new();
    s.select_all(&v);
    assert_eq!(sorted(&s), [A, B, C]);
    assert_eq!(s.anchor(), Some(A));

    let h2: u32 = 102;
    let v = VecView::with_headers(&[h0, A, h2, B, C], &[h0, h2]);
    let mut s = Selection::new();
    s.click(A);
    s.move_focus(1, false, &v);
    assert_eq!(s.focus(), Some(B));
    assert_eq!(sorted(&s), [B]);
    s.move_focus(-1, false, &v);
    assert_eq!(s.focus(), Some(A));
    // Up from the first process stays there: the header above is not selectable.
    s.move_focus(-1, false, &v);
    assert_eq!(s.focus(), Some(A));
    assert_eq!(sorted(&s), [A]);

    // Shift ranges skip headers.
    s.move_focus(2, true, &v);
    assert_eq!(s.focus(), Some(C));
    assert_eq!(sorted(&s), [A, B, C]);
    s.click(A);
    s.shift_click(C, &v);
    assert_eq!(sorted(&s), [A, B, C]);
    s.click(C);
    s.ctrl_shift_click(A, &v);
    assert_eq!(sorted(&s), [A, B, C]);

    // No focus: the first Down lands on the first selectable row, not the header.
    let mut s = Selection::new();
    s.move_focus(1, false, &v);
    assert_eq!(s.focus(), Some(A));

    // Home lands on the first process, not the header.
    s.click(C);
    s.move_edge(false, true, &v);
    assert_eq!(s.focus(), Some(A));
    assert_eq!(sorted(&s), [A, B, C]);
    assert!(!s.contains(&h0) && !s.contains(&h2));

    // A view with no selectable rows changes nothing.
    let only_headers = VecView::with_headers(&[h0, h2], &[h0, h2]);
    let mut s = Selection::new();
    s.move_focus(1, false, &only_headers);
    s.select_all(&only_headers);
    assert_eq!(s.len(), 0);
    assert_eq!(s.focus(), None);
}

#[test]
fn selection_drops_exited_after_linger() {
    let v = VecView::new(&[A, B, C, D]);
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(C);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(C)));

    // C exited and its linger time is over: the backing data no longer has it.
    let alive = VecView::new(&[A, B, D]);
    s.retain(|id| alive.index_of(*id).is_some());
    assert_eq!(sorted(&s), [B]);
    assert_eq!(s.focus(), None);
    assert_eq!(s.anchor(), None);

    // Focus and anchor on ids that stay are kept.
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(C);
    s.ctrl_click(C);
    assert_eq!((s.anchor(), s.focus()), (Some(C), Some(C)));
    s.ctrl_click(B);
    s.ctrl_click(B);
    s.retain(|&id| id != C);
    assert_eq!(sorted(&s), [B]);
    assert_eq!((s.anchor(), s.focus()), (Some(B), Some(B)));
    assert!(v.index_of(C).is_some());
}

#[test]
fn selection_hidden_by_filter_not_counted() {
    let full = VecView::new(&[A, B, C, D]);
    let mut s = Selection::new();
    s.click(B);
    s.ctrl_click(C);

    let filtered = VecView::new(&[A, B, D]);
    assert_eq!(s.visible_count(&filtered), 1);
    assert!(s.contains(&C));
    assert_eq!(s.len(), 2);
    assert_eq!(s.in_view_order(&filtered).as_slice(), [B]);

    // A gesture against the filtered view does not touch the hidden id.
    s.ctrl_click(D);
    assert!(s.contains(&C));

    // The filter is cleared: C reappears in display order.
    assert_eq!(s.visible_count(&full), 3);
    assert_eq!(s.in_view_order(&full).as_slice(), [B, C, D]);
}

#[test]
fn page_and_edge_moves() {
    let rows: Vec<u32> = (0..30).collect();
    let v = VecView::new(&rows);
    let viewport_rows = 10;
    let mut s = Selection::new();
    s.click(0);

    // PgDn with a viewport of 10 rows moves 9 rows.
    s.move_page(viewport_rows - 1, true, false, &v);
    assert_eq!(s.focus(), Some(9));
    assert_eq!(sorted(&s), [9]);
    assert_eq!(s.anchor(), Some(9));

    // Shift+PgDn extends from the anchor.
    s.move_page(viewport_rows - 1, true, true, &v);
    assert_eq!(s.focus(), Some(18));
    assert_eq!(sorted(&s), (9..=18).collect::<Vec<_>>());
    assert_eq!(s.anchor(), Some(9));

    // PgDn clamps to the last row, PgUp clamps to the first.
    s.move_page(viewport_rows - 1, true, false, &v);
    s.move_page(viewport_rows - 1, true, false, &v);
    assert_eq!(s.focus(), Some(29));
    s.move_page(viewport_rows - 1, false, false, &v);
    assert_eq!(s.focus(), Some(20));
    s.move_page(100, false, false, &v);
    assert_eq!(s.focus(), Some(0));

    // End goes to the last selectable row; Shift+Home extends back from the anchor.
    s.click(5);
    s.move_edge(true, false, &v);
    assert_eq!(s.focus(), Some(29));
    assert_eq!(sorted(&s), [29]);
    s.move_edge(false, true, &v);
    assert_eq!(s.focus(), Some(0));
    assert_eq!(s.anchor(), Some(29));
    assert_eq!(s.len(), 30);

    // Shift+End from a click extends from the anchor.
    s.click(25);
    s.move_edge(true, true, &v);
    assert_eq!(sorted(&s), (25..=29).collect::<Vec<_>>());

    // End skips a trailing group header; PgDn landing on a header moves on to a process.
    let mut rows: Vec<u32> = (0..12).collect();
    rows.push(100);
    let v = VecView::with_headers(&rows, &[9, 100]);
    let mut s = Selection::new();
    s.move_edge(true, false, &v);
    assert_eq!(s.focus(), Some(11));
    s.click(0);
    s.move_page(viewport_rows - 1, true, false, &v);
    assert_eq!(s.focus(), Some(10));
    s.move_page(viewport_rows - 1, false, false, &v);
    assert_eq!(s.focus(), Some(1));
}
