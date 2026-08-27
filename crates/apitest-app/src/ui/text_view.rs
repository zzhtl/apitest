use std::ops::Range;

use eframe::egui::{self, Color32, RichText, text::LayoutJob};

use crate::theme::UiExt;

/// Longest display row, in bytes, before a line is broken into chunks. A
/// minified 10 MiB JSON is one line; without chunking the virtualized viewer
/// would degenerate back into laying out the whole document.
pub(crate) const MAX_ROW_BYTES: usize = 4096;

/// Split `text` into display rows: one per line, long lines chunked at
/// `MAX_ROW_BYTES` (snapped back to a character boundary). Ranges never
/// include the trailing newline.
pub(crate) fn split_display_rows(text: &str) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content_len = line.strip_suffix('\n').unwrap_or(line).len();
        let mut start = offset;
        let line_end = offset + content_len;
        loop {
            let mut end = (start + MAX_ROW_BYTES).min(line_end);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            rows.push(start..end);
            if end >= line_end {
                break;
            }
            start = end;
        }
        offset += line.len();
    }
    if rows.is_empty() {
        rows.push(0..0);
    }
    rows
}

/// Read-only, row-virtualized text viewer for large bodies.
///
/// Only the visible rows are laid out, so a 10 MiB response scrolls smoothly;
/// the trade-off is that text selection works per row rather than across the
/// whole document (copy/save buttons still cover the full content).
pub(crate) fn virtual_text_view(
    ui: &mut egui::Ui,
    text: &str,
    rows: &[Range<usize>],
    matches: &[Range<usize>],
    highlight: Color32,
) {
    let text_color = ui.palette().text;
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::both().auto_shrink([false; 2]).show_rows(
        ui,
        row_height,
        rows.len(),
        |ui, visible| {
            for index in visible {
                let row = rows[index].clone();
                let content = &text[row.clone()];
                let intersecting = intersecting_matches(matches, &row);
                if intersecting.is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(content).monospace().color(text_color))
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                } else {
                    let job = row_job(content, row.start, intersecting, ui, text_color, highlight);
                    ui.add(
                        egui::Label::new(job)
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Extend),
                    );
                }
            }
        },
    );
}

fn intersecting_matches<'a>(matches: &'a [Range<usize>], row: &Range<usize>) -> &'a [Range<usize>] {
    // `matches` is sorted by start; narrow to the slice overlapping this row.
    let first = matches.partition_point(|span| span.end <= row.start);
    let last = matches.partition_point(|span| span.start < row.end);
    &matches[first..last]
}

fn row_job(
    content: &str,
    row_start: usize,
    matches: &[Range<usize>],
    ui: &egui::Ui,
    text_color: Color32,
    highlight: Color32,
) -> LayoutJob {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = egui::TextFormat::simple(font.clone(), text_color);
    let mut marked = plain.clone();
    marked.background = highlight;
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let mut cursor = 0;
    for span in matches {
        let start = span.start.saturating_sub(row_start).min(content.len());
        let end = span.end.saturating_sub(row_start).min(content.len());
        let (start, end) = (snap(content, start), snap(content, end));
        if cursor < start {
            job.append(&content[cursor..start], 0.0, plain.clone());
        }
        if start < end {
            job.append(&content[start..end], 0.0, marked.clone());
        }
        cursor = cursor.max(end);
    }
    if cursor < content.len() {
        job.append(&content[cursor..], 0.0, plain);
    }
    job
}

fn snap(content: &str, mut index: usize) -> usize {
    while index > 0 && !content.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Byte ranges of every case-insensitive occurrence of `needle`, matching the
/// semantics the per-frame scan used to have (indices from the lowercased
/// text applied to the original).
pub(crate) fn match_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    if needle.is_empty() {
        return Vec::new();
    }
    let lowered = text.to_lowercase();
    let needle = needle.to_lowercase();
    let mut ranges = Vec::new();
    let mut index = 0;
    while let Some(found) = lowered[index..].find(&needle) {
        let start = index + found;
        let end = start + needle.len();
        ranges.push(start..end);
        index = end;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::{MAX_ROW_BYTES, match_ranges, split_display_rows};

    #[test]
    fn rows_follow_lines_and_exclude_newlines() {
        let rows = split_display_rows("ab\ncd\n\nef");
        assert_eq!(rows, vec![0..2, 3..5, 6..6, 7..9]);
    }

    #[test]
    fn empty_text_still_produces_one_row() {
        assert_eq!(split_display_rows(""), vec![0..0]);
    }

    #[test]
    fn overlong_lines_are_chunked_on_character_boundaries() {
        // 3-byte characters that do not divide MAX_ROW_BYTES evenly.
        let text = "汉".repeat(2000);
        let rows = split_display_rows(&text);
        assert!(rows.len() > 1);
        for row in &rows {
            assert!(row.len() <= MAX_ROW_BYTES);
            assert!(text.is_char_boundary(row.start) && text.is_char_boundary(row.end));
        }
        assert_eq!(rows.last().unwrap().end, text.len());
        assert_eq!(rows.iter().map(|row| row.len()).sum::<usize>(), text.len());
    }

    #[test]
    fn match_ranges_are_case_insensitive_and_sorted() {
        let ranges = match_ranges("Alpha beta ALPHA", "alpha");
        assert_eq!(ranges, vec![0..5, 11..16]);
    }
}
