//! Editor services for PhonoScript source.
//!
//! This module deliberately consumes the same token stream as the language
//! front end.  The editor therefore cannot drift into a second, informal
//! spelling of the language merely to provide syntax colour.  Trivia is
//! recovered from the gaps between tokens because comments are intentionally
//! discarded by the parser.

use std::ops::Range;

use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextFormat};
use egui::text::LayoutJob;

use crate::phonoscript_analysis::AnalysisDiagnostic;
use crate::phonoscript_frontend::{self, Diagnostic, Severity, Token, TokenKind, TokenTag};
use crate::phonoscript_runtime::RuntimeDiagnostic;

const SOURCE_FONT_SIZE: f32 = 12.5;
const SOURCE_EDITOR_HORIZONTAL_PADDING: f32 = 20.0;
const SOURCE_EDITOR_FALLBACK_GLYPH_WIDTH: f32 = SOURCE_FONT_SIZE * 0.64;
const TAB_COLUMNS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceClass {
    Plain,
    Keyword,
    Literal,
    Callable,
    Comment,
    Operator,
    Error,
}

/// One diagnostic overlay for the source editor. The adapters keep the layout
/// API independent of the three diagnostic structures while retaining their
/// exact source span and severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorDiagnosticSpan {
    pub span: phonoscript_frontend::Span,
    pub severity: Severity,
}

impl From<&Diagnostic> for EditorDiagnosticSpan {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            span: diagnostic.primary,
            severity: diagnostic.severity,
        }
    }
}

impl From<&AnalysisDiagnostic> for EditorDiagnosticSpan {
    fn from(diagnostic: &AnalysisDiagnostic) -> Self {
        Self {
            span: diagnostic.primary,
            severity: diagnostic.severity,
        }
    }
}

impl From<&RuntimeDiagnostic> for EditorDiagnosticSpan {
    fn from(diagnostic: &RuntimeDiagnostic) -> Self {
        Self {
            span: diagnostic.primary,
            severity: diagnostic.severity,
        }
    }
}

/// Parse `source` once for an editor refresh and return its complete live
/// lexical/parser diagnostics. Static-analysis diagnostics are appended by
/// the application after the corresponding analysis pass.
pub fn live_frontend_diagnostics(source: &str) -> Vec<Diagnostic> {
    phonoscript_frontend::parse(source).diagnostics
}

/// Return only diagnostics whose spans belong to the currently edited source.
/// Imported-module diagnostics keep their own source names in the Problems
/// list, but must never underline unrelated byte coordinates in this buffer.
pub fn diagnostic_spans_for_source(
    diagnostics: &[RuntimeDiagnostic],
    source_name: &str,
) -> Vec<EditorDiagnosticSpan> {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.source_name == source_name)
        .map(EditorDiagnosticSpan::from)
        .collect()
}

/// Return a stable editor width that fits the longest source line without
/// wrapping while never becoming narrower than the visible viewport. Glyph
/// advances come from egui's active monospace font, so Unicode fallback glyphs
/// are included in the calculation instead of being approximated from bytes.
pub fn source_editor_content_width(
    source: &str,
    viewport_width: f32,
    mut glyph_width: impl FnMut(char) -> f32,
) -> f32 {
    let usable_width = |width: f32| {
        if width.is_finite() && width > 0.0 {
            width
        } else {
            SOURCE_EDITOR_FALLBACK_GLYPH_WIDTH
        }
    };
    let space_width = usable_width(glyph_width(' '));
    let widest_line = source
        .split('\n')
        .map(|line| {
            let mut width = 0.0_f32;
            let mut column = 0_usize;
            for character in line.strip_suffix('\r').unwrap_or(line).chars() {
                if character == '\t' {
                    let spaces = TAB_COLUMNS - (column % TAB_COLUMNS);
                    width += space_width * spaces as f32;
                    column += spaces;
                } else {
                    width += usable_width(glyph_width(character));
                    column += 1;
                }
            }
            width
        })
        .fold(0.0_f32, f32::max);
    viewport_width
        .max(widest_line + SOURCE_EDITOR_HORIZONTAL_PADDING)
        .ceil()
}

pub fn source_font_id() -> FontId {
    FontId::new(SOURCE_FONT_SIZE, FontFamily::Monospace)
}

/// Construct a professional, restrained syntax-coloured layout using the
/// production scanner. The exact source spelling is retained byte for byte.
pub fn layout_job(source: &str, wrap_width: f32, dark: bool) -> LayoutJob {
    layout_job_with_diagnostics(source, wrap_width, dark, &[])
}

/// Construct the production editor layout and overlay additional static or
/// runtime diagnostics. Frontend diagnostics are collected from the same
/// single scan/parse pass that supplies syntax classes. Only errors are
/// underlined; warnings retain their syntax colour. Spans are clipped safely
/// to UTF-8 boundaries, including zero-width error anchors, and source bytes
/// are never inserted, removed, or rewritten.
pub fn layout_job_with_diagnostics(
    source: &str,
    wrap_width: f32,
    dark: bool,
    diagnostics: &[EditorDiagnosticSpan],
) -> LayoutJob {
    let parsed = phonoscript_frontend::parse(source);
    let error_ranges = normalized_error_ranges(
        source,
        parsed
            .diagnostics
            .iter()
            .map(EditorDiagnosticSpan::from)
            .chain(diagnostics.iter().copied()),
    );
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;

    let mut cursor = 0_usize;
    for (index, token) in parsed.tokens.iter().enumerate() {
        let start = token.span.start.byte.min(source.len());
        let end = token.span.end.byte.min(source.len());
        if cursor < start {
            append_trivia(
                &mut job,
                &source[cursor..start],
                cursor,
                dark,
                &error_ranges,
            );
        }
        if start < end {
            let next_significant = parsed.tokens[index + 1..]
                .iter()
                .find(|next| !matches!(next.kind, TokenKind::Newline | TokenKind::Eof));
            let class = token_class(token, next_significant);
            append_with_error_ranges(
                &mut job,
                &source[start..end],
                start,
                class,
                dark,
                &error_ranges,
            );
        }
        cursor = cursor.max(end);
    }
    if cursor < source.len() {
        append_trivia(&mut job, &source[cursor..], cursor, dark, &error_ranges);
    }
    job
}

fn token_class(token: &Token, next: Option<&Token>) -> SourceClass {
    match token.kind.tag() {
        TokenTag::Let
        | TokenTag::Var
        | TokenTag::Fn
        | TokenTag::If
        | TokenTag::Else
        | TokenTag::While
        | TokenTag::For
        | TokenTag::In
        | TokenTag::Return
        | TokenTag::Import
        | TokenTag::Export
        | TokenTag::From
        | TokenTag::As
        | TokenTag::Command
        | TokenTag::And
        | TokenTag::Or
        | TokenTag::Not => SourceClass::Keyword,
        TokenTag::True | TokenTag::False | TokenTag::Null | TokenTag::Number | TokenTag::Text => {
            SourceClass::Literal
        }
        TokenTag::Identifier
            if next.is_some_and(|next| matches!(next.kind, TokenKind::LeftParen)) =>
        {
            SourceClass::Callable
        }
        TokenTag::Plus
        | TokenTag::Dot
        | TokenTag::Minus
        | TokenTag::Star
        | TokenTag::Slash
        | TokenTag::Percent
        | TokenTag::Bang
        | TokenTag::BangEqual
        | TokenTag::Equal
        | TokenTag::EqualEqual
        | TokenTag::Greater
        | TokenTag::GreaterEqual
        | TokenTag::Less
        | TokenTag::LessEqual
        | TokenTag::AndAnd
        | TokenTag::OrOr
        | TokenTag::Arrow => SourceClass::Operator,
        TokenTag::Error => SourceClass::Error,
        _ => SourceClass::Plain,
    }
}

fn normalized_error_ranges(
    source: &str,
    diagnostics: impl IntoIterator<Item = EditorDiagnosticSpan>,
) -> Vec<Range<usize>> {
    let mut ranges = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .filter_map(|diagnostic| normalized_error_range(source, diagnostic.span))
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn normalized_error_range(source: &str, span: phonoscript_frontend::Span) -> Option<Range<usize>> {
    if source.is_empty() {
        return None;
    }
    let mut start = span.start.byte.min(source.len());
    let mut end = span.end.byte.min(source.len());
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    while start > 0 && !source.is_char_boundary(start) {
        start -= 1;
    }
    while end < source.len() && !source.is_char_boundary(end) {
        end += 1;
    }
    if start == end {
        if end < source.len() {
            end += source[end..].chars().next()?.len_utf8();
        } else {
            start = source[..start]
                .char_indices()
                .last()
                .map_or(0, |(index, _)| index);
        }
    }
    (start < end).then_some(start..end)
}

fn append_with_error_ranges(
    job: &mut LayoutJob,
    text: &str,
    absolute_start: usize,
    class: SourceClass,
    dark: bool,
    error_ranges: &[Range<usize>],
) {
    if text.is_empty() {
        return;
    }
    let absolute_end = absolute_start + text.len();
    let mut boundaries = vec![absolute_start, absolute_end];
    for range in error_ranges {
        if range.start < absolute_end && absolute_start < range.end {
            boundaries.push(range.start.max(absolute_start));
            boundaries.push(range.end.min(absolute_end));
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for bounds in boundaries.windows(2) {
        let start = bounds[0];
        let end = bounds[1];
        let erroneous = error_ranges
            .iter()
            .any(|range| range.start < end && start < range.end);
        append(
            job,
            &text[start - absolute_start..end - absolute_start],
            if erroneous { SourceClass::Error } else { class },
            dark,
        );
    }
}

fn append_trivia(
    job: &mut LayoutJob,
    source: &str,
    absolute_start: usize,
    dark: bool,
    error_ranges: &[Range<usize>],
) {
    let mut cursor = 0_usize;
    while cursor < source.len() {
        let remainder = &source[cursor..];
        if remainder.starts_with('#') || remainder.starts_with("//") {
            let end = remainder.find(['\n', '\r']).unwrap_or(remainder.len());
            append_with_error_ranges(
                job,
                &remainder[..end],
                absolute_start + cursor,
                SourceClass::Comment,
                dark,
                error_ranges,
            );
            cursor += end;
            continue;
        }
        if remainder.starts_with("/*") {
            let mut depth = 1_usize;
            let mut end = 2_usize;
            while end < remainder.len() && depth > 0 {
                let nested = &remainder[end..];
                if nested.starts_with("/*") {
                    depth += 1;
                    end += 2;
                } else if nested.starts_with("*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += nested.chars().next().map_or(1, char::len_utf8);
                }
            }
            append_with_error_ranges(
                job,
                &remainder[..end],
                absolute_start + cursor,
                SourceClass::Comment,
                dark,
                error_ranges,
            );
            cursor += end;
            continue;
        }
        let mut end = remainder
            .char_indices()
            .nth(1)
            .map_or(remainder.len(), |(index, _)| index);
        while end < remainder.len() {
            let tail = &remainder[end..];
            if tail.starts_with('#') || tail.starts_with("//") || tail.starts_with("/*") {
                break;
            }
            end += tail.chars().next().map_or(1, char::len_utf8);
        }
        append_with_error_ranges(
            job,
            &remainder[..end],
            absolute_start + cursor,
            SourceClass::Plain,
            dark,
            error_ranges,
        );
        cursor += end;
    }
}

fn append(job: &mut LayoutJob, text: &str, class: SourceClass, dark: bool) {
    if text.is_empty() {
        return;
    }
    let (plain, keyword, literal, callable, comment, operator, error) = if dark {
        (
            Color32::from_rgb(220, 224, 228),
            Color32::from_rgb(158, 188, 209),
            Color32::from_rgb(199, 177, 139),
            Color32::from_rgb(180, 201, 216),
            Color32::from_rgb(135, 151, 143),
            Color32::from_rgb(172, 181, 188),
            Color32::from_rgb(234, 168, 168),
        )
    } else {
        (
            Color32::from_rgb(31, 38, 45),
            Color32::from_rgb(39, 79, 106),
            Color32::from_rgb(112, 74, 39),
            Color32::from_rgb(47, 87, 112),
            Color32::from_rgb(82, 102, 89),
            Color32::from_rgb(78, 88, 96),
            Color32::from_rgb(126, 45, 45),
        )
    };
    let color = match class {
        SourceClass::Plain => plain,
        SourceClass::Keyword => keyword,
        SourceClass::Literal => literal,
        SourceClass::Callable => callable,
        SourceClass::Comment => comment,
        SourceClass::Operator => operator,
        SourceClass::Error => error,
    };
    let mut format = TextFormat {
        font_id: source_font_id(),
        color,
        ..TextFormat::default()
    };
    if class == SourceClass::Keyword {
        format.italics = false;
    }
    if class == SourceClass::Comment {
        format.italics = true;
    }
    if class == SourceClass::Error {
        format.underline = Stroke::new(1.0_f32, error);
        format.background = if dark {
            Color32::from_rgb(67, 39, 42)
        } else {
            Color32::from_rgb(249, 232, 232)
        };
    }
    job.append(text, 0.0, format);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::ConvalgenDocument, phonoscript_analysis, phonoscript_runtime};

    fn span_is_underlined(job: &LayoutJob, span: phonoscript_frontend::Span) -> bool {
        job.sections.iter().any(|section| {
            section.byte_range.start <= span.start.byte
                && span.end.byte <= section.byte_range.end
                && section.format.underline.width > 0.0
        }) || (span.start.byte..span.end.byte).all(|byte| {
            job.sections.iter().any(|section| {
                section.byte_range.start <= byte
                    && byte < section.byte_range.end
                    && section.format.underline.width > 0.0
            })
        })
    }

    #[test]
    fn layout_retains_source_byte_for_byte() {
        let source = "let σ = 1/3; // exact\nprint(\"σ\");\n";
        let job = layout_job(source, 600.0, false);
        assert_eq!(job.text, source);
    }

    #[test]
    fn source_editor_width_tracks_the_longest_visual_line_and_tab_stops() {
        let source = "short\n1234\t5678\n123456789012\n";
        let width = source_editor_content_width(source, 60.0, |_| 8.0);
        assert_eq!(width, 116.0);
        assert_eq!(source_editor_content_width("tiny", 240.0, |_| 8.0), 240.0);
    }

    #[test]
    fn source_editor_width_counts_unicode_glyph_advances_not_utf8_bytes() {
        let source = "let σ = \"候補\"";
        let width = source_editor_content_width(source, 1.0, |character| {
            if matches!(character, '候' | '補') {
                14.0
            } else {
                7.0
            }
        });
        let expected_glyph_width = source.chars().fold(0.0_f32, |width, character| {
            width
                + if matches!(character, '候' | '補') {
                    14.0
                } else {
                    7.0
                }
        });
        assert_eq!(width, (expected_glyph_width + 20.0).ceil());
    }

    #[test]
    fn nested_comments_are_one_editor_class_without_losing_text() {
        let source = "/* outer /* nested */ done */\nlet x = 1;";
        let job = layout_job(source, 600.0, true);
        assert_eq!(job.text, source);
        assert!(job.sections.len() >= 4);
    }

    #[test]
    fn null_and_quoted_record_keys_use_literal_colouring() {
        let source = "let r = {\"text key\": null, σ: 1/3};\n";
        let job = layout_job(source, 600.0, false);
        assert_eq!(job.text, source);
        for spelling in ["\"text key\"", "null", "1/3"] {
            let start = source.find(spelling).expect("spelling in source");
            let end = start + spelling.len();
            let section = job
                .sections
                .iter()
                .find(|section| section.byte_range.start <= start && section.byte_range.end >= end)
                .expect("token has one layout section");
            assert_eq!(section.format.color, Color32::from_rgb(112, 74, 39));
        }
    }

    #[test]
    fn module_declaration_keywords_use_the_language_keyword_colour() {
        let source = "import { alpha as beta } from \"./library.phont\"\nexport let gamma = beta\n";
        let job = layout_job(source, 600.0, false);
        assert_eq!(job.text, source);
        for spelling in ["import", "as", "from", "export"] {
            let start = source.find(spelling).expect("module keyword in source");
            let end = start + spelling.len();
            let section = job
                .sections
                .iter()
                .find(|section| section.byte_range.start <= start && section.byte_range.end >= end)
                .expect("module keyword has one layout section");
            assert_eq!(section.format.color, Color32::from_rgb(39, 79, 106));
        }
    }

    #[test]
    fn imported_diagnostics_never_overlay_the_entry_buffer() {
        let mut diagnostics = phonoscript_runtime::check_named("main.phont", "missing_main\n");
        diagnostics.extend(phonoscript_runtime::check_named(
            "modules/library.phont",
            "missing_library\n",
        ));
        let spans = diagnostic_spans_for_source(&diagnostics, "main.phont");
        assert!(!spans.is_empty());
        assert_eq!(
            spans.len(),
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.source_name == "main.phont")
                .count()
        );
        let imported_span = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.source_name == "modules/library.phont")
            .expect("imported diagnostic")
            .primary;
        assert!(!spans.iter().any(|overlay| overlay.span == imported_span));
    }

    #[test]
    fn malformed_source_gets_an_error_section_and_live_diagnostic() {
        let source = "let x = @;";
        let job = layout_job(source, 600.0, false);
        assert_eq!(job.text, source);
        assert!(!live_frontend_diagnostics(source).is_empty());
        assert!(
            job.sections.iter().any(|section| {
                section.format.underline.color == Color32::from_rgb(126, 45, 45)
            })
        );
    }

    #[test]
    fn additional_static_error_spans_are_underlined_without_rewriting_source() {
        let source = "let fixed = 1\nfixed = 2\n";
        let parsed = phonoscript_frontend::parse(source);
        let report = phonoscript_analysis::analyze(&parsed.program);
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|item| item.code.as_str() == "PSA1004")
            .expect("immutable-assignment diagnostic");
        let overlays = [EditorDiagnosticSpan::from(diagnostic)];
        let job = layout_job_with_diagnostics(source, 600.0, false, &overlays);
        assert_eq!(job.text, source);
        assert!(span_is_underlined(&job, diagnostic.primary));
    }

    #[test]
    fn additional_runtime_error_spans_are_underlined_without_retokenizing_rules() {
        let source = "let record = {a: 1}\nrecord[\"missing\"]\n";
        let result = phonoscript_runtime::run(source, &ConvalgenDocument::blank());
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|item| item.code == "PSR0403")
            .expect("missing-field runtime diagnostic");
        let overlays = [EditorDiagnosticSpan::from(diagnostic)];
        let job = layout_job_with_diagnostics(source, 600.0, true, &overlays);
        assert_eq!(job.text, source);
        assert!(span_is_underlined(&job, diagnostic.primary));
        assert!(
            job.sections.iter().any(|section| {
                section.format.underline.color == Color32::from_rgb(234, 168, 168)
            })
        );
    }

    #[test]
    fn warning_overlays_are_not_underlined_and_zero_width_errors_get_an_anchor() {
        let source = "let x = 1";
        let parsed = phonoscript_frontend::parse(source);
        let let_span = parsed.tokens[0].span;
        let eof_span = parsed.tokens.last().expect("EOF token").span;
        let overlays = [
            EditorDiagnosticSpan {
                span: let_span,
                severity: Severity::Warning,
            },
            EditorDiagnosticSpan {
                span: eof_span,
                severity: Severity::Error,
            },
        ];
        let job = layout_job_with_diagnostics(source, 600.0, false, &overlays);
        assert_eq!(job.text, source);
        assert!(!span_is_underlined(&job, let_span));
        assert!(job.sections.iter().any(|section| {
            section.byte_range.end == source.len() && section.format.underline.width > 0.0
        }));
    }
}
