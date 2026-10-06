//! Table selection keyed by row identity, never by index (UI-PRC-008, ADR-009).
//!
//! The set, anchor and focus hold row ids, so a selection survives re-sorts, filter changes and
//! view-mode changes. Every gesture that needs row order takes a [`SelectionView`], the rows in
//! current display order. Rows the view marks as not selectable (group headers, D-011) are never
//! added to the set by a view-based gesture. The plain `click`, `ctrl_click` and `right_click`
//! take no view, so the caller must not pass a group header id to them.

use std::hash::Hash;

use rustc_hash::FxHashSet;
use smallvec::SmallVec;

/// Selected row ids plus the anchor (start of Shift ranges) and the focus (keyboard cursor).
#[derive(Debug, Clone)]
pub struct Selection<R: Copy + Eq + Hash> {
    set: FxHashSet<R>,
    anchor: Option<R>,
    focus: Option<R>,
}

/// The view the gesture is computed against: rows in current display order.
pub trait SelectionView<R> {
    /// Number of rows in the view.
    fn len(&self) -> usize;
    /// Row id at display index `ix`.
    fn id(&self, ix: usize) -> R;
    /// Display index of `id`, or `None` when it is not in the view (filtered or collapsed).
    fn index_of(&self, id: R) -> Option<usize>;
    /// False for rows that can never be selected, such as group headers.
    fn selectable(&self, ix: usize) -> bool;
}

impl<R: Copy + Eq + Hash> Default for Selection<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Copy + Eq + Hash> Selection<R> {
    pub fn new() -> Self {
        Self {
            set: FxHashSet::default(),
            anchor: None,
            focus: None,
        }
    }

    /// Click: set = {id}, anchor = focus = id.
    pub fn click(&mut self, id: R) {
        self.set.clear();
        self.set.insert(id);
        self.anchor = Some(id);
        self.focus = Some(id);
    }

    /// Ctrl+click: toggle id. Focus moves to id. The anchor moves to id only when it was added.
    pub fn ctrl_click(&mut self, id: R) {
        if self.set.remove(&id) {
            self.focus = Some(id);
        } else {
            self.set.insert(id);
            self.anchor = Some(id);
            self.focus = Some(id);
        }
    }

    /// Shift+click: set = selectable rows from the anchor to id in view order. Focus moves to id
    /// and the anchor stays. With no anchor in the view this is a plain click.
    pub fn shift_click(&mut self, id: R, v: &impl SelectionView<R>) {
        let Some((a, b)) = self.anchor_range(id, v) else {
            self.click(id);
            return;
        };
        self.set.clear();
        self.insert_range(a, b, v);
        self.focus = Some(id);
    }

    /// Ctrl+Shift+click: add the selectable rows from the anchor to id to the set. Focus moves to
    /// id and the anchor stays. With no anchor in the view this is a Ctrl+click.
    pub fn ctrl_shift_click(&mut self, id: R, v: &impl SelectionView<R>) {
        let Some((a, b)) = self.anchor_range(id, v) else {
            self.ctrl_click(id);
            return;
        };
        self.insert_range(a, b, v);
        self.focus = Some(id);
    }

    /// Right-click: an unselected row becomes the only selection, a selected row leaves the set
    /// as it is. Returns true when the set changed.
    pub fn right_click(&mut self, id: R) -> bool {
        if self.set.contains(&id) {
            return false;
        }
        self.click(id);
        true
    }

    /// Click on the empty body area: nothing selected, no anchor, no focus.
    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
        self.focus = None;
    }

    /// Ctrl+A: set = every selectable row in the view, anchor = the first of them. Hidden ids
    /// leave the set. Focus stays, or goes to the anchor when there was none.
    pub fn select_all(&mut self, v: &impl SelectionView<R>) {
        self.set.clear();
        let Some(first) = first_selectable(v) else {
            self.anchor = None;
            return;
        };
        self.insert_range(first, v.len() - 1, v);
        let first = v.id(first);
        self.anchor = Some(first);
        self.focus = self.focus.or(Some(first));
    }

    /// Up (negative delta) or Down: move focus over `delta` selectable rows, clamped to the first
    /// or last selectable row. Without `extend`, set = {focus} and anchor = focus. With `extend`,
    /// set = anchor..focus. With no focus in the view, focus goes to the first selectable row.
    pub fn move_focus(&mut self, delta: i32, extend: bool, v: &impl SelectionView<R>) {
        let target = match self.focus_index(v) {
            None => first_selectable(v),
            Some(mut ix) => {
                for _ in 0..delta.unsigned_abs() {
                    let next = if delta > 0 {
                        next_selectable(v, ix + 1)
                    } else {
                        ix.checked_sub(1)
                            .and_then(|start| prev_selectable(v, start))
                    };
                    match next {
                        Some(n) => ix = n,
                        None => break,
                    }
                }
                Some(ix)
            }
        };
        self.land(target, extend, v);
    }

    /// PgUp or PgDn: move focus by `rows` display rows (viewport rows - 1), clamped to the first
    /// or last selectable row. A landing on a group header moves on to the next selectable row
    /// in the same direction. Set and anchor follow the same rules as [`Self::move_focus`].
    pub fn move_page(&mut self, rows: usize, down: bool, extend: bool, v: &impl SelectionView<R>) {
        let target = match self.focus_index(v) {
            None => first_selectable(v),
            Some(ix) => {
                let last = v.len() - 1;
                if down {
                    let t = ix.saturating_add(rows).min(last);
                    next_selectable(v, t).or_else(|| prev_selectable(v, t))
                } else {
                    let t = ix.saturating_sub(rows);
                    prev_selectable(v, t).or_else(|| next_selectable(v, t))
                }
            }
        };
        self.land(target, extend, v);
    }

    /// Home or End: focus goes to the first or last selectable row. Set and anchor follow the
    /// same rules as [`Self::move_focus`].
    pub fn move_edge(&mut self, end: bool, extend: bool, v: &impl SelectionView<R>) {
        let target = if end {
            v.len()
                .checked_sub(1)
                .and_then(|last| prev_selectable(v, last))
        } else {
            first_selectable(v)
        };
        self.land(target, extend, v);
    }

    /// Ctrl+Space: toggle the focused row, the same as a Ctrl+click on it.
    pub fn toggle_focus(&mut self) {
        if let Some(f) = self.focus {
            self.ctrl_click(f);
        }
    }

    /// Move focus without changing the set or the anchor (jump helpers).
    pub fn set_focus_only(&mut self, id: R) {
        self.focus = Some(id);
    }

    /// Drop ids that no longer exist, such as processes that exited after the linger time
    /// (PROC-043). An anchor or focus on a dropped id becomes `None`.
    pub fn retain(&mut self, mut keep: impl FnMut(&R) -> bool) {
        self.set.retain(|id| keep(id));
        if self.anchor.is_some_and(|a| !keep(&a)) {
            self.anchor = None;
        }
        if self.focus.is_some_and(|f| !keep(&f)) {
            self.focus = None;
        }
    }

    pub fn contains(&self, id: &R) -> bool {
        self.set.contains(id)
    }

    /// Every selected id, hidden ones included, in no particular order.
    pub fn ids(&self) -> impl Iterator<Item = R> + '_ {
        self.set.iter().copied()
    }

    pub fn anchor(&self) -> Option<R> {
        self.anchor
    }

    pub fn focus(&self) -> Option<R> {
        self.focus
    }

    /// Number of selected ids, hidden ones included.
    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Number of selected ids present in the view. Ids hidden by the filter or a collapsed
    /// parent are not counted.
    pub fn visible_count(&self, v: &impl SelectionView<R>) -> usize {
        self.set
            .iter()
            .filter(|&&id| v.index_of(id).is_some())
            .count()
    }

    /// Selected ids present in the view, in display order. Hidden ids are left out.
    pub fn in_view_order(&self, v: &impl SelectionView<R>) -> SmallVec<[R; 8]> {
        let mut rows: SmallVec<[(usize, R); 8]> = self
            .set
            .iter()
            .filter_map(|&id| v.index_of(id).map(|ix| (ix, id)))
            .collect();
        rows.sort_unstable_by_key(|&(ix, _)| ix);
        rows.into_iter().map(|(_, id)| id).collect()
    }

    /// Display indexes of the anchor and id, lowest first, when both are in the view.
    fn anchor_range(&self, id: R, v: &impl SelectionView<R>) -> Option<(usize, usize)> {
        let a = v.index_of(self.anchor?)?;
        let b = v.index_of(id)?;
        Some((a.min(b), a.max(b)))
    }

    fn focus_index(&self, v: &impl SelectionView<R>) -> Option<usize> {
        v.index_of(self.focus?)
    }

    /// Add the selectable rows in display indexes `lo..=hi` to the set.
    fn insert_range(&mut self, lo: usize, hi: usize, v: &impl SelectionView<R>) {
        for ix in lo..=hi {
            if v.selectable(ix) {
                self.set.insert(v.id(ix));
            }
        }
    }

    /// Finish a keyboard move onto display index `target`. Without `extend`, set = {focus} and
    /// anchor = focus. With `extend`, set = anchor..focus; a missing anchor becomes the row the
    /// focus left, or the new focus when there was none.
    fn land(&mut self, target: Option<usize>, extend: bool, v: &impl SelectionView<R>) {
        let Some(target) = target else {
            return;
        };
        let id = v.id(target);
        if !extend {
            self.click(id);
            return;
        }
        let anchor_ix = self
            .anchor
            .and_then(|a| v.index_of(a))
            .or_else(|| self.focus_index(v))
            .unwrap_or(target);
        self.anchor = Some(v.id(anchor_ix));
        self.set.clear();
        self.insert_range(anchor_ix.min(target), anchor_ix.max(target), v);
        self.focus = Some(id);
    }
}

fn first_selectable<R>(v: &impl SelectionView<R>) -> Option<usize> {
    next_selectable(v, 0)
}

/// First selectable display index at or after `start`.
fn next_selectable<R>(v: &impl SelectionView<R>, start: usize) -> Option<usize> {
    (start..v.len()).find(|&ix| v.selectable(ix))
}

/// Last selectable display index at or before `start`.
fn prev_selectable<R>(v: &impl SelectionView<R>, start: usize) -> Option<usize> {
    (0..=start.min(v.len().checked_sub(1)?))
        .rev()
        .find(|&ix| v.selectable(ix))
}
