use eframe::egui::{self, Color32, text::ByteIndex, text::LayoutJob};
use egui_extras::syntax_highlighting::{CodeTheme, highlight};

use crate::draft::BodyMode;
use crate::theme::{Palette, UiExt};

/// Which syntax definition to use, in `syntect`'s extension vocabulary.
pub(crate) fn body_syntax(mode: BodyMode) -> &'static str {
    match mode {
        BodyMode::Json => "json",
        BodyMode::Xml => "xml",
        _ => "txt",
    }
}

/// The names the active environment can resolve, used to tell a live template
/// apart from a typo.
#[derive(Default, Clone)]
pub(crate) struct KnownVariables {
    names: std::collections::BTreeSet<String>,
}

impl KnownVariables {
    pub(crate) fn from_names(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            names: names.into_iter().collect(),
        }
    }

    fn knows(&self, name: &str) -> bool {
        self.names.contains(name.trim())
    }
}

/// Byte ranges of `{{name}}` templates, paired with whether the name resolves.
fn variable_spans(text: &str, known: &KnownVariables) -> Vec<(std::ops::Range<usize>, bool)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut index = 0;
    while let Some(start) = text[index..].find("{{") {
        let start = index + start;
        let Some(end) = text[start + 2..].find("}}") else {
            break;
        };
        let end = start + 2 + end + 2;
        let name = &text[start + 2..end - 2];
        // A template never spans lines; an unclosed brace pair is just text.
        if !name.contains('\n') {
            spans.push((start..end, known.knows(name)));
        }
        index = end;
        if index >= bytes.len() {
            break;
        }
    }
    spans
}

/// Re-colour the template spans on top of the syntax highlighting.
///
/// Splitting existing sections keeps the highlighter's colours everywhere else,
/// which is why this runs as a second pass rather than replacing it.
fn recolor_variables(job: &mut LayoutJob, known: &KnownVariables, palette: Palette) {
    let spans = variable_spans(&job.text, known);
    if spans.is_empty() {
        return;
    }
    let mut sections = Vec::with_capacity(job.sections.len() + spans.len() * 2);
    for section in job.sections.drain(..) {
        let (section_start, section_end) = (section.byte_range.start.0, section.byte_range.end.0);
        let mut cursor = section_start;
        for (span, resolved) in &spans {
            if span.end <= section_start || span.start >= section_end {
                continue;
            }
            let start = span.start.max(section_start);
            let end = span.end.min(section_end);
            if cursor < start {
                let mut head = section.clone();
                head.byte_range = ByteIndex(cursor)..ByteIndex(start);
                sections.push(head);
            }
            let mut marked = section.clone();
            marked.byte_range = ByteIndex(start)..ByteIndex(end);
            marked.format.color = if *resolved {
                palette.success
            } else {
                palette.danger
            };
            sections.push(marked);
            cursor = end;
        }
        if cursor < section_end {
            let mut tail = section.clone();
            tail.byte_range = ByteIndex(cursor)..ByteIndex(section_end);
            sections.push(tail);
        }
    }
    job.sections = sections;
}

fn layout(
    ui: &egui::Ui,
    text: &str,
    syntax: &str,
    known: &KnownVariables,
    wrap_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let theme = CodeTheme::from_style(ui.style());
    let mut job = highlight(ui.ctx(), ui.style(), &theme, text, syntax);
    recolor_variables(&mut job, known, ui.palette());
    job.wrap.max_width = wrap_width;
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// A syntax-highlighted editor that also marks `{{variables}}`.
pub(crate) fn code_editor(
    ui: &mut egui::Ui,
    text: &mut String,
    syntax: &str,
    known: &KnownVariables,
    wrap: bool,
) -> egui::Response {
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        layout(
            ui,
            buffer.as_str(),
            syntax,
            known,
            if wrap { wrap_width } else { f32::INFINITY },
        )
    };
    ui.add_sized(
        ui.available_size(),
        egui::TextEdit::multiline(text)
            .code_editor()
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter),
    )
}

/// The read-only counterpart, used for response bodies.
///
/// A `TextEdit` rather than a `Label` so the text stays selectable and shares
/// the highlighting path. The caller owns `buffer` (refreshed only when the
/// body changes, not per frame) and precomputes the search `matches`.
pub(crate) fn code_view(
    ui: &mut egui::Ui,
    buffer: &mut String,
    syntax: &str,
    matches: &[std::ops::Range<usize>],
    wrap: bool,
) -> egui::Response {
    let highlight_color = ui.palette().primary_soft;
    let mut layouter = |ui: &egui::Ui, source: &dyn egui::TextBuffer, wrap_width: f32| {
        let theme = CodeTheme::from_style(ui.style());
        let mut job = highlight(ui.ctx(), ui.style(), &theme, source.as_str(), syntax);
        apply_match_ranges(&mut job, matches, highlight_color);
        job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };
    ui.add_sized(
        ui.available_size(),
        egui::TextEdit::multiline(buffer)
            .code_editor()
            .desired_width(f32::INFINITY)
            .layouter(&mut layouter),
    )
}

/// Paint a background behind every section that falls inside a match span.
///
/// Both lists are ordered by byte position (`matches` never overlap), so one
/// merge pass suffices; this runs inside the layouter, i.e. per frame.
fn apply_match_ranges(
    job: &mut LayoutJob,
    matches: &[std::ops::Range<usize>],
    highlight_color: Color32,
) {
    if matches.is_empty() {
        return;
    }
    let mut next = 0;
    for section in &mut job.sections {
        let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
        while next < matches.len() && matches[next].end <= start {
            next += 1;
        }
        let Some(span) = matches.get(next) else {
            break;
        };
        if span.start <= start && end <= span.end {
            section.format.background = highlight_color;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{KnownVariables, variable_spans};

    #[test]
    fn templates_are_split_into_resolved_and_unknown() {
        let known = KnownVariables::from_names(["base_url".to_owned()]);
        let spans = variable_spans("{{base_url}}/users/{{missing}}", &known);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0], (0..12, true));
        assert_eq!(spans[1], (19..30, false));
    }

    #[test]
    fn an_unclosed_template_is_left_alone() {
        let known = KnownVariables::default();
        assert!(variable_spans("{{oops", &known).is_empty());
    }

    #[test]
    fn a_template_never_spans_lines() {
        let known = KnownVariables::default();
        assert!(variable_spans("{{a\nb}}", &known).is_empty());
    }
}
