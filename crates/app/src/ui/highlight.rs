//! Highlighting of the query's text terms in the Name and Path cells.
//!
//! This is a display aid, not the matcher: terms are plain substrings found with ASCII
//! case-folding. Filters (`ext:`, `size:` …), regular expressions and negated terms are not
//! highlighted.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontSelection, Sense, Ui, vec2};

/// Literal substrings of the text terms of `texts`, deduplicated and ASCII-lowercased.
pub(crate) fn terms(texts: &[String], case_sensitive: bool, regex: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if regex {
        return out;
    }
    for text in texts {
        for token in tokens(text) {
            let mut word = token.as_str();
            if word.starts_with('!') {
                continue;
            }
            if let Some((head, rest)) = word.split_once(':') {
                match head.to_ascii_lowercase().as_str() {
                    "path" | "file" | "folder" | "case" => word = rest,
                    "ext" | "size" | "dm" | "regex" | "vol" => continue,
                    _ => {}
                }
            }
            for part in word.split(['*', '?', '|']) {
                let part = part.trim_matches('"');
                if part.is_empty() {
                    continue;
                }
                let part = if case_sensitive {
                    part.to_owned()
                } else {
                    part.to_ascii_lowercase()
                };
                if !out.contains(&part) {
                    out.push(part);
                }
            }
        }
    }
    out
}

/// Splits on whitespace outside double quotes; the quotes are removed.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Sorted, non-overlapping byte ranges of `text` covered by any of `terms`.
pub(crate) fn ranges(text: &str, terms: &[String], case_sensitive: bool) -> Vec<Range<usize>> {
    let hay = text.as_bytes();
    let mut found: Vec<Range<usize>> = Vec::new();
    for term in terms {
        let needle = term.as_bytes();
        if needle.is_empty() || needle.len() > hay.len() {
            continue;
        }
        for start in 0..=hay.len() - needle.len() {
            let window = &hay[start..start + needle.len()];
            let hit = if case_sensitive {
                window == needle
            } else {
                window.eq_ignore_ascii_case(needle)
            };
            // A valid UTF-8 needle can only match on char boundaries of a valid UTF-8 haystack.
            if hit {
                found.push(start..start + needle.len());
            }
        }
    }
    found.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(found.len());
    for r in found {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// Horizontal offset of the second pass that fakes a bold face (egui bundles no bold font).
const EMBOLDEN: f32 = 0.7;

/// A single-line label drawing the matched ranges of `text` in white and emboldened, the rest in
/// `base`, truncated with an ellipsis when it does not fit.
pub(crate) fn label(ui: &mut Ui, text: &str, matched: &[Range<usize>], base: Color32) {
    let font = FontSelection::Default.resolve(ui.style());
    let max_width = ui.available_width();
    let build = |rest: Color32| {
        let mut job = LayoutJob::default();
        let mut pos = 0;
        let hit = TextFormat::simple(font.clone(), Color32::WHITE);
        let miss = TextFormat::simple(font.clone(), rest);
        for r in matched {
            if r.start > pos {
                job.append(&text[pos..r.start], 0.0, miss.clone());
            }
            job.append(&text[r.clone()], 0.0, hit.clone());
            pos = r.end;
        }
        if pos < text.len() {
            job.append(&text[pos..], 0.0, miss);
        }
        job.wrap.max_width = max_width;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        job
    };
    let (galley, ghost) = ui.fonts_mut(|f| {
        (
            f.layout_job(build(base)),
            f.layout_job(build(Color32::TRANSPARENT)),
        )
    });
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), text));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    painter.galley(rect.min, galley, base);
    painter.galley(rect.min + vec2(EMBOLDEN, 0.0), ghost, base);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn terms_skip_filters_and_negation() {
        let t = terms(
            &s(&["Report ext:pdf !draft \"my doc\" path:src*lib"]),
            false,
            false,
        );
        assert_eq!(t, ["report", "my doc", "src", "lib"]);
    }

    #[test]
    fn ranges_are_case_insensitive_and_merged() {
        let r = ranges("Report_report", &s(&["report", "ort_r"]), false);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].start, r[0].end), (0, 13));
        assert_eq!(ranges("abc", &s(&["B"]), true), Vec::<Range<usize>>::new());
    }
}
