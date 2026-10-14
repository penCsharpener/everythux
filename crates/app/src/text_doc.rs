//! The text of a text preview, indexed for virtualised rendering, plus the user's selection.
//!
//! Rendering only lays out the rows in view, so the text is split into display rows up front:
//! one per line, or several per line when wrapping. Monospace makes that a column count instead
//! of a layout pass, which keeps an 8 MiB preview inside the frame budget. Everything here is
//! plain data so it can be tested without a UI.

use std::ops::Range;

use everythux_preview::{TextEncoding, more_text_available};

/// Width of a tab in columns, matching egui's default `FontTweak::tab_size`.
const TAB_COLUMNS: usize = 4;
/// Without wrapping, longer lines still continue on a new row after this many columns. Laying
/// out (and painting) a megabyte-long minified line every frame would blow the frame budget.
pub(crate) const MAX_ROW_COLUMNS: usize = 1000;
/// Narrowest wrap width in columns.
const MIN_WRAP_COLUMNS: usize = 8;

/// One display row: a byte range of the text without its line break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl Row {
    pub(crate) fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// A text preview being shown.
#[derive(Debug, Clone)]
pub(crate) struct TextDoc {
    text: String,
    /// Byte offset where each line starts; the first is 0.
    line_starts: Vec<u32>,
    pub(crate) encoding: TextEncoding,
    /// File size in bytes.
    pub(crate) total_bytes: u64,
    /// File offset just after the text: where "Show more" continues.
    pub(crate) end_offset: u64,
    rows: Vec<Row>,
    /// Columns per row the rows were built for.
    row_columns: usize,
    /// Widest row in columns.
    max_columns: usize,
    /// Selected byte range as (anchor, head); both on char boundaries.
    selection: Option<(usize, usize)>,
}

impl TextDoc {
    pub(crate) fn new(text: String, encoding: TextEncoding, total_bytes: u64, end_offset: u64) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(memchr::memchr_iter(b'\n', text.as_bytes()).map(|i| (i + 1) as u32));
        Self {
            text,
            line_starts,
            encoding,
            total_bytes,
            end_offset,
            rows: Vec::new(),
            row_columns: 0,
            max_columns: 0,
            selection: None,
        }
    }

    /// Appends the next chunk of the file.
    pub(crate) fn append(&mut self, chunk: &str, total_bytes: u64, end_offset: u64) {
        let old_len = self.text.len();
        self.text.push_str(chunk);
        self.line_starts
            .extend(memchr::memchr_iter(b'\n', chunk.as_bytes()).map(|i| (old_len + i + 1) as u32));
        self.total_bytes = total_bytes;
        self.end_offset = end_offset;
        if self.row_columns > 0 {
            // The last old line may continue in the new chunk: rebuild from its start.
            let from_line = self
                .line_starts
                .partition_point(|&s| (s as usize) <= old_len)
                .saturating_sub(1);
            self.build_rows_from(from_line);
        }
    }

    /// Whether "Show more" can load another chunk.
    pub(crate) fn more_available(&self) -> bool {
        more_text_available(self.end_offset, self.total_bytes)
    }

    pub(crate) fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Splits the text into rows of at most `columns` columns (clamped to
    /// `MIN_WRAP_COLUMNS..=MAX_ROW_COLUMNS`). Cheap when nothing changed.
    pub(crate) fn layout(&mut self, columns: usize) {
        let columns = columns.clamp(MIN_WRAP_COLUMNS, MAX_ROW_COLUMNS);
        if columns != self.row_columns {
            self.row_columns = columns;
            self.rows.clear();
            self.max_columns = 0;
            self.build_rows_from(0);
        }
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Widest row in columns.
    pub(crate) fn max_columns(&self) -> usize {
        self.max_columns
    }

    /// The text of `row`.
    pub(crate) fn row_text(&self, row: Row) -> &str {
        &self.text[row.range()]
    }

    /// 1-based line number of `row` if it is the first row of its line (wrapped continuation rows
    /// get none).
    pub(crate) fn line_number(&self, row: Row) -> Option<usize> {
        let line = self.line_starts.partition_point(|&s| s <= row.start);
        (self.line_starts[line - 1] == row.start).then_some(line)
    }

    fn build_rows_from(&mut self, line: usize) {
        let keep_until = self.line_starts[line];
        let keep = self.rows.partition_point(|r| r.start < keep_until);
        self.rows.truncate(keep);
        let columns = self.row_columns;
        let bytes = self.text.as_bytes();
        for (i, &start) in self.line_starts.iter().enumerate().skip(line) {
            let start = start as usize;
            let mut end = self
                .line_starts
                .get(i + 1)
                .map_or(bytes.len(), |&next| next as usize - 1);
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            let line_text = &self.text[start..end];
            if line_text.is_ascii() && !line_text.contains('\t') {
                // Fast path: one byte per column.
                let mut s = start;
                loop {
                    let e = (s + columns).min(end);
                    self.rows.push(row(s, e));
                    self.max_columns = self.max_columns.max(e - s);
                    if e >= end {
                        break;
                    }
                    s = e;
                }
                continue;
            }
            if line_text.len() <= columns && !line_text.contains('\t') {
                // Every character but a tab takes at most as many columns as it has bytes.
                self.rows.push(row(start, end));
                self.max_columns = self.max_columns.max(line_text.chars().map(char_columns).sum());
                continue;
            }
            let mut row_start = start;
            let mut used = 0;
            for (offset, c) in line_text.char_indices() {
                let width = char_columns(c);
                if used + width > columns && used > 0 {
                    self.rows.push(row(row_start, start + offset));
                    self.max_columns = self.max_columns.max(used);
                    row_start = start + offset;
                    used = 0;
                }
                used += width;
            }
            self.rows.push(row(row_start, end));
            self.max_columns = self.max_columns.max(used);
        }
    }

    // ---- selection -----------------------------------------------------------------------

    /// Starts a selection at byte `at` (a click).
    pub(crate) fn select_from(&mut self, at: usize) {
        let at = self.floor_boundary(at);
        self.selection = Some((at, at));
    }

    /// Moves the selection head to byte `at` (a drag or Shift+click).
    pub(crate) fn select_to(&mut self, at: usize) {
        let at = self.floor_boundary(at);
        let anchor = self.selection.map_or(at, |(anchor, _)| anchor);
        self.selection = Some((anchor, at));
    }

    pub(crate) fn select_all(&mut self) {
        self.selection = Some((0, self.text.len()));
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// The selected byte range, if it is not empty.
    pub(crate) fn selection(&self) -> Option<Range<usize>> {
        let (a, b) = self.selection?;
        (a != b).then(|| a.min(b)..a.max(b))
    }

    pub(crate) fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    fn floor_boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
}

fn row(start: usize, end: usize) -> Row {
    Row {
        start: start as u32,
        end: end as u32,
    }
}

/// Columns a character takes in a monospace font: tabs are [`TAB_COLUMNS`] wide, East Asian wide
/// characters and most emoji two.
pub(crate) fn char_columns(c: char) -> usize {
    match c {
        '\t' => TAB_COLUMNS,
        '\u{1100}'..='\u{115F}'
        | '\u{2E80}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7A3}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE30}'..='\u{FE4F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}'
        | '\u{1F300}'..='\u{1FAFF}'
        | '\u{20000}'..='\u{3FFFD}' => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> TextDoc {
        let len = text.len() as u64;
        TextDoc::new(text.to_owned(), TextEncoding::Utf8, len, len)
    }

    fn row_texts(doc: &TextDoc) -> Vec<&str> {
        doc.rows().iter().map(|&r| doc.row_text(r)).collect()
    }

    #[test]
    fn lines_become_rows_without_their_line_breaks() {
        let mut d = doc("one\r\ntwo\n\nthree");
        d.layout(MAX_ROW_COLUMNS);
        assert_eq!(row_texts(&d), ["one", "two", "", "three"]);
        assert_eq!(d.line_count(), 4);
        let numbers: Vec<_> = d.rows().iter().map(|&r| d.line_number(r)).collect();
        assert_eq!(numbers, [Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(d.max_columns(), 5);
    }

    #[test]
    fn a_trailing_newline_ends_with_an_empty_row() {
        let mut d = doc("a\n");
        d.layout(80);
        assert_eq!(row_texts(&d), ["a", ""]);
    }

    #[test]
    fn wrapping_splits_long_lines_and_numbers_only_their_first_row() {
        let mut d = doc("abcdefghijKLMNOPQRSTuvw\nxy");
        d.layout(10);
        assert_eq!(row_texts(&d), ["abcdefghij", "KLMNOPQRST", "uvw", "xy"]);
        let numbers: Vec<_> = d.rows().iter().map(|&r| d.line_number(r)).collect();
        assert_eq!(numbers, [Some(1), None, None, Some(2)]);
    }

    #[test]
    fn wrapping_counts_tabs_and_wide_characters() {
        let mut d = doc("\tab\tcd\nｗｉｄｅｗｉｄｅ\näöüäöüäöü");
        d.layout(8);
        assert_eq!(
            row_texts(&d),
            ["\tab", "\tcd", "ｗｉｄｅ", "ｗｉｄｅ", "äöüäöüäö", "ü"]
        );
    }

    #[test]
    fn no_wrap_still_cuts_absurdly_long_lines() {
        let long = "x".repeat(MAX_ROW_COLUMNS * 2 + 5);
        let mut d = doc(&long);
        d.layout(usize::MAX);
        assert_eq!(d.rows().len(), 3);
        assert_eq!(d.max_columns(), MAX_ROW_COLUMNS);
    }

    #[test]
    fn appending_continues_the_last_line() {
        let mut d = TextDoc::new("first\nsec".to_owned(), TextEncoding::Utf8, 100, 9);
        d.layout(80);
        assert!(d.more_available());
        d.append("ond\nthird\n", 100, 19);
        assert_eq!(row_texts(&d), ["first", "second", "third", ""]);
        assert_eq!((d.end_offset, d.line_count()), (19, 4));
        let mut fresh = doc("first\nsecond\nthird\n");
        fresh.layout(80);
        assert_eq!(d.rows(), fresh.rows(), "same rows as laying out the whole text");
    }

    #[test]
    fn selection_is_ordered_and_snaps_to_char_boundaries() {
        let mut d = doc("grüße\nwelt");
        assert_eq!(d.selected_text(), None);
        d.select_from(8);
        d.select_to(3); // inside "ü": snaps back to its start
        assert_eq!(d.selected_text(), Some("üße\n"));
        d.select_all();
        assert_eq!(d.selected_text(), Some("grüße\nwelt"));
        d.select_from(2);
        assert_eq!(d.selected_text(), None, "a click is an empty selection");
        d.select_to(100);
        assert_eq!(d.selected_text(), Some("üße\nwelt"));
        d.clear_selection();
        assert_eq!(d.selection(), None);
    }
}
