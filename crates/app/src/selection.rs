//! Row selection for the result table.
//!
//! Selections are stored as sorted, disjoint row ranges so that `Shift+Click` over a million rows
//! costs O(1) instead of materialising a million indices.

use std::ops::Range;

/// A set of selected rows plus the anchor (for `Shift+Click`) and the cursor (keyboard focus).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Selection {
    /// Sorted, non-overlapping, non-adjacent ranges.
    ranges: Vec<Range<usize>>,
    anchor: Option<usize>,
    cursor: Option<usize>,
}

impl Selection {
    /// Whether nothing is selected.
    pub(crate) fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Number of selected rows.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ranges.iter().map(ExactSizeIterator::len).sum()
    }

    /// Whether `row` is selected.
    pub(crate) fn contains(&self, row: usize) -> bool {
        let i = self.ranges.partition_point(|r| r.end <= row);
        self.ranges.get(i).is_some_and(|r| r.start <= row)
    }

    /// The row with keyboard focus (the last one clicked or moved to).
    pub(crate) fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Selected rows in ascending order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = usize> + '_ {
        self.ranges.iter().flat_map(Clone::clone)
    }

    /// A selection of exactly `rows`. `cursor` (also used as anchor) falls back to the first row
    /// when it is `None` or not a valid row of the new list.
    pub(crate) fn from_rows(rows: impl IntoIterator<Item = usize>, cursor: Option<usize>) -> Self {
        let mut sel = Self::default();
        for row in rows {
            sel.add_range(row..row + 1);
        }
        let cursor = cursor.or_else(|| sel.ranges.first().map(|r| r.start));
        sel.anchor = cursor;
        sel.cursor = cursor;
        sel
    }

    /// Deselects everything and forgets anchor and cursor.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Plain click: only `row` is selected.
    pub(crate) fn select_only(&mut self, row: usize) {
        self.ranges.clear();
        self.ranges.push(row..row + 1);
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    /// `Ctrl+Click`: flips `row` and makes it the anchor.
    pub(crate) fn toggle(&mut self, row: usize) {
        if self.contains(row) {
            self.remove_row(row);
        } else {
            self.add_range(row..row + 1);
        }
        self.anchor = Some(row);
        self.cursor = Some(row);
    }

    /// `Shift+Click`: selects from the anchor to `row`, replacing the previous selection.
    /// Without an anchor it behaves like [`Self::select_only`].
    pub(crate) fn select_range_to(&mut self, row: usize) {
        let anchor = self.anchor.unwrap_or(row);
        let (lo, hi) = (anchor.min(row), anchor.max(row));
        self.ranges.clear();
        self.ranges.push(lo..hi + 1);
        self.anchor = Some(anchor);
        self.cursor = Some(row);
    }

    /// Keyboard movement. With `extend` the selection grows from the anchor, else it collapses to `row`.
    pub(crate) fn move_to(&mut self, row: usize, extend: bool) {
        if extend && self.anchor.is_some() {
            self.select_range_to(row);
        } else {
            self.select_only(row);
        }
    }

    fn add_range(&mut self, new: Range<usize>) {
        let start = self.ranges.partition_point(|r| r.end < new.start);
        let mut merged = new;
        let mut end = start;
        while let Some(r) = self.ranges.get(end) {
            if r.start > merged.end {
                break;
            }
            merged.start = merged.start.min(r.start);
            merged.end = merged.end.max(r.end);
            end += 1;
        }
        self.ranges.splice(start..end, [merged]);
    }

    fn remove_row(&mut self, row: usize) {
        let i = self.ranges.partition_point(|r| r.end <= row);
        let Some(r) = self.ranges.get(i).cloned() else {
            return;
        };
        if r.start > row {
            return;
        }
        let mut replacement = Vec::with_capacity(2);
        if r.start < row {
            replacement.push(r.start..row);
        }
        if row + 1 < r.end {
            replacement.push(row + 1..r.end);
        }
        self.ranges.splice(i..=i, replacement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(sel: &Selection) -> Vec<usize> {
        sel.rows().collect()
    }

    #[test]
    fn click_selects_a_single_row() {
        let mut sel = Selection::default();
        sel.select_only(4);
        sel.select_only(7);
        assert_eq!(rows(&sel), [7]);
        assert_eq!(sel.cursor(), Some(7));
    }

    #[test]
    fn ctrl_click_toggles() {
        let mut sel = Selection::default();
        sel.select_only(1);
        sel.toggle(3);
        sel.toggle(2);
        assert_eq!(rows(&sel), [1, 2, 3]);
        sel.toggle(2);
        assert_eq!(rows(&sel), [1, 3]);
        assert!(sel.contains(3));
        assert!(!sel.contains(2));
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn shift_click_selects_a_range_from_the_anchor() {
        let mut sel = Selection::default();
        sel.select_only(5);
        sel.select_range_to(2);
        assert_eq!(rows(&sel), [2, 3, 4, 5]);
        sel.select_range_to(8);
        assert_eq!(rows(&sel), [5, 6, 7, 8], "anchor stays at 5");
    }

    #[test]
    fn huge_ranges_are_cheap() {
        let mut sel = Selection::default();
        sel.select_only(0);
        sel.select_range_to(2_000_000);
        assert_eq!(sel.len(), 2_000_001);
        assert!(sel.contains(1_999_999));
        assert!(!sel.contains(2_000_001));
    }

    #[test]
    fn keyboard_extend_and_collapse() {
        let mut sel = Selection::default();
        sel.move_to(3, false);
        sel.move_to(4, true);
        sel.move_to(5, true);
        assert_eq!(rows(&sel), [3, 4, 5]);
        sel.move_to(4, false);
        assert_eq!(rows(&sel), [4]);
    }

    #[test]
    fn from_rows_merges_and_sets_the_cursor() {
        let sel = Selection::from_rows([5, 3, 4, 9, 3], Some(9));
        assert_eq!(rows(&sel), [3, 4, 5, 9]);
        assert_eq!(sel.cursor(), Some(9));
        let sel = Selection::from_rows([7, 2], None);
        assert_eq!(sel.cursor(), Some(2), "falls back to the first row");
        assert!(Selection::from_rows([], None).is_empty());
    }

    #[test]
    fn removing_the_middle_splits_a_range() {
        let mut sel = Selection::default();
        sel.select_only(0);
        sel.select_range_to(4);
        sel.toggle(2);
        assert_eq!(rows(&sel), [0, 1, 3, 4]);
    }
}
