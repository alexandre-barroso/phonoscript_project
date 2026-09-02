use std::fmt::Write as _;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::engine::{ComparisonStatus, SecondOrderResult, SerialResult, resolved_violation};
use crate::exact::NumericScalar;
use crate::model::{
    Candidate, ConvalgenDocument, EvaluatorKind, PlotKind, QueryKind, ResponseDomain,
    SecondOrderLayout, Tableau,
};
use crate::phonological_engine::PhonologicalEngine;

// Native equivalents of secondordertableau.sty's restrained grayscale tokens.
// SVG is the single native vector source; no TeX is emitted or invoked.
const INK: &str = "#171717";
const LINE: &str = "#2e2e2e";
const SOFT: &str = "#f0f0f0";
const MID: &str = "#949494";
const WHITE: &str = "#ffffff";
// These are the internal family names of the bundled, redistributable faces.
// resvg/svg2pdf resolve by the OpenType family name; aliases in @font-face are
// not sufficient at the native conversion boundary.
const TEXT_FAMILY: &str = "'Noto Sans', 'Noto Sans Arabic', 'emoji', sans-serif";
const PADDING: f32 = 20.0;
const BOUNDARY_GAP: f32 = 4.0;
const MAX_PNG_SIDE: u32 = 32_768;
const MAX_PNG_PIXELS: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Svg,
    Png,
    Pdf,
}

#[derive(Debug, Clone)]
pub struct PublicationTile {
    pub row: usize,
    pub column: usize,
    pub rows: usize,
    pub columns: usize,
    pub svg: String,
}

impl ExportFormat {
    pub const ALL: [Self; 3] = [Self::Svg, Self::Png, Self::Pdf];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Svg => "SVG",
            Self::Png => "PNG",
            Self::Pdf => "PDF",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Svg => "svg",
            Self::Png => "png",
            Self::Pdf => "pdf",
        }
    }
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml_id(prefix: &str, value: &str) -> String {
    let mut result = String::with_capacity(prefix.len() + value.len() + 1);
    result.push_str(prefix);
    result.push('-');
    let mut separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
            separator = false;
        } else if !separator {
            result.push('-');
            separator = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result == prefix {
        result.push_str("-item");
    }
    result
}

fn svg_start(width: f32, height: f32, title: &str, description: &str) -> String {
    let text_font = BASE64.encode(ttf_noto_sans::REGULAR);
    let bold_font = BASE64.encode(ttf_noto_sans::BOLD);
    let arabic_font = BASE64.encode(rwml_fonts::noto_sans_arabic_subset());
    let symbol_font = BASE64.encode(epaint_default_fonts::EMOJI_ICON);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.1}" height="{height:.1}" viewBox="0 0 {width:.1} {height:.1}" role="img" aria-labelledby="export-title export-description" data-crop="content" data-padding="{PADDING:.1}">
<title id="export-title">{}</title>
<desc id="export-description">{}</desc>
<metadata>PhonoScript GUI native vector export; crop-to-content; PNG scale is declared at the write boundary.</metadata>
<defs>
  <style><![CDATA[
    @font-face {{ font-family: 'Noto Sans'; src: url(data:font/ttf;base64,{text_font}) format('truetype'); font-style: normal; font-weight: 400; }}
    @font-face {{ font-family: 'Noto Sans'; src: url(data:font/ttf;base64,{bold_font}) format('truetype'); font-style: normal; font-weight: 700; }}
    @font-face {{ font-family: 'Noto Sans Arabic'; src: url(data:font/ttf;base64,{arabic_font}) format('truetype'); font-style: normal; font-weight: 400; }}
    @font-face {{ font-family: 'emoji'; src: url(data:font/ttf;base64,{symbol_font}) format('truetype'); font-style: normal; font-weight: 400; }}
    text {{ font-family: {TEXT_FAMILY}; }}
  ]]></style>
  <pattern id="hatch-diagonal" width="7" height="7" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
    <line x1="0" y1="0" x2="0" y2="7" stroke="{LINE}" stroke-width="1"/>
  </pattern>
  <pattern id="hatch-cross" width="8" height="8" patternUnits="userSpaceOnUse">
    <path d="M0 0L8 8M8 0L0 8" stroke="{MID}" stroke-width="0.8"/>
  </pattern>
</defs>
<rect id="background" x="0" y="0" width="{width:.1}" height="{height:.1}" fill="{WHITE}"/>
"##,
        xml(title),
        xml(description)
    )
}

fn group_start(svg: &mut String, id: &str, class: &str, extra: &str) {
    let _ = writeln!(
        svg,
        r#"<g id="{}" class="{}" {}>"#,
        xml(id),
        xml(class),
        extra
    );
}

fn group_end(svg: &mut String) {
    svg.push_str("</g>\n");
}

fn title_node(svg: &mut String, value: &str) {
    let _ = writeln!(svg, "<title>{}</title>", xml(value));
}

fn text(svg: &mut String, x: f32, y: f32, size: f32, weight: u16, value: &str) {
    let _ = writeln!(
        svg,
        r#"<text x="{x:.1}" y="{y:.1}" font-size="{size:.1}" font-weight="{weight}" fill="{INK}">{}</text>"#,
        xml(value)
    );
}

fn right_text(svg: &mut String, x: f32, y: f32, size: f32, weight: u16, value: &str) {
    let _ = writeln!(
        svg,
        r#"<text x="{x:.1}" y="{y:.1}" text-anchor="end" font-size="{size:.1}" font-weight="{weight}" fill="{INK}">{}</text>"#,
        xml(value)
    );
}

fn centered(svg: &mut String, x: f32, y: f32, size: f32, weight: u16, value: &str) {
    let _ = writeln!(
        svg,
        r#"<text x="{x:.1}" y="{y:.1}" text-anchor="middle" dominant-baseline="middle" font-size="{size:.1}" font-weight="{weight}" fill="{INK}">{}</text>"#,
        xml(value)
    );
}

fn line(svg: &mut String, x1: f32, y1: f32, x2: f32, y2: f32, width: f32) {
    let _ = writeln!(
        svg,
        r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{LINE}" stroke-width="{width:.1}"/>"#
    );
}

fn dotted_line(svg: &mut String, x: f32, y1: f32, y2: f32) {
    let _ = writeln!(
        svg,
        r#"<line x1="{x:.1}" y1="{y1:.1}" x2="{x:.1}" y2="{y2:.1}" stroke="{LINE}" stroke-width="1" stroke-dasharray="2 3" data-boundary="tied"/>"#
    );
}

fn strict_boundary_line(svg: &mut String, x: f32, y1: f32, y2: f32) {
    let _ = writeln!(
        svg,
        r#"<line x1="{x:.1}" y1="{y1:.1}" x2="{x:.1}" y2="{y2:.1}" stroke="{LINE}" stroke-width="1" data-boundary="strict"/>"#
    );
}

fn jagged_line(svg: &mut String, x: f32, y1: f32, y2: f32) {
    let mut path = format!("M{x:.1} {y1:.1}");
    let mut y = y1;
    let mut right = true;
    while y < y2 {
        y = (y + 5.0).min(y2);
        let dx = if right { 2.0 } else { -2.0 };
        let _ = write!(path, " L{:.1} {y:.1}", x + dx);
        right = !right;
    }
    let _ = writeln!(
        svg,
        r#"<path d="{path}" fill="none" stroke="{LINE}" stroke-width="1" data-boundary="independent"/>"#
    );
}

fn rect(svg: &mut String, x: f32, y: f32, width: f32, height: f32, fill: &str) {
    let _ = writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" fill="{fill}"/>"#
    );
}

fn measure_text(value: &str, size: f32, weight: u16) -> f32 {
    let data = if weight >= 600 {
        ttf_noto_sans::BOLD
    } else {
        ttf_noto_sans::REGULAR
    };
    let Some(face) = rustybuzz::Face::from_slice(data, 0) else {
        return value.graphemes(true).count() as f32 * size * 0.6;
    };
    let units_per_em = (face.units_per_em() as f32).max(1.0);
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(value);
    let shaped = rustybuzz::shape(&face, &[], buffer);
    let advance = shaped
        .glyph_positions()
        .iter()
        .map(|position| position.x_advance as f32)
        .sum::<f32>();
    advance.abs() * size / units_per_em
}

fn break_token(token: &str, width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for grapheme in token.graphemes(true) {
        let proposal = format!("{line}{grapheme}");
        if !line.is_empty() && measure_text(&proposal, size, 400) > width {
            lines.push(std::mem::take(&mut line));
        }
        line.push_str(grapheme);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_text(value: &str, width: f32, size: f32) -> Vec<String> {
    let width = width.max(size);
    let mut result = Vec::new();
    for paragraph in value.split('\n') {
        if paragraph.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let fragments = if measure_text(word, size, 400) > width {
                break_token(word, width, size)
            } else {
                vec![word.to_owned()]
            };
            for fragment in fragments {
                let proposal = if line.is_empty() {
                    fragment.clone()
                } else {
                    format!("{line} {fragment}")
                };
                if !line.is_empty() && measure_text(&proposal, size, 400) > width {
                    result.push(std::mem::take(&mut line));
                    line = fragment;
                } else {
                    line = proposal;
                }
            }
        }
        if !line.is_empty() {
            result.push(line);
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

fn text_lines(
    svg: &mut String,
    x: f32,
    first_baseline: f32,
    line_height: f32,
    size: f32,
    weight: u16,
    lines: &[String],
) {
    for (index, value) in lines.iter().enumerate() {
        text(
            svg,
            x,
            first_baseline + index as f32 * line_height,
            size,
            weight,
            value,
        );
    }
}

fn centered_lines(
    svg: &mut String,
    x: f32,
    center_y: f32,
    line_height: f32,
    size: f32,
    weight: u16,
    lines: &[String],
) {
    let start = center_y - (lines.len().saturating_sub(1) as f32 * line_height) / 2.0;
    for (index, value) in lines.iter().enumerate() {
        centered(
            svg,
            x,
            start + index as f32 * line_height,
            size,
            weight,
            value,
        );
    }
}

fn candidate_label(index: usize) -> String {
    if index < 26 {
        format!("{}.", char::from(b'a' + index as u8))
    } else {
        format!("{}.", index + 1)
    }
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

const fn query_code(query: QueryKind) -> &'static str {
    match query {
        QueryKind::WinnerSet | QueryKind::SurfaceWinnerSet => "WIN",
        QueryKind::CompleteOrder => "ORD",
        QueryKind::ProbabilityLaw => "PROB",
        QueryKind::CandidateSupport => "SUPPORT",
    }
}

fn metric_headers(evaluator: EvaluatorKind) -> &'static [&'static str] {
    match evaluator {
        EvaluatorKind::Ot => &[],
        EvaluatorKind::HarmonicGrammar => &["cost ↓"],
        EvaluatorKind::MaxEnt => &["E ↓", "ρ", "u", "P"],
    }
}

fn tableau_convention(
    tableau: &Tableau,
    document: &ConvalgenDocument,
    evaluator: EvaluatorKind,
    normalizer: Option<f64>,
) -> String {
    match evaluator {
        EvaluatorKind::Ot => {
            let mut convention = format!(
                "left-to-right dominance · tie policy: {} · ☞ selected winner",
                tableau.tie_policy_kind().label()
            );
            let unresolved = tableau
                .temperature_scalar_or(&document.temperature)
                .to_f64_center()
                .ok()
                .and_then(|temperature| {
                    PhonologicalEngine::new()
                        .evaluate(tableau, evaluator, temperature)
                        .ok()
                })
                .is_some_and(|evaluation| {
                    evaluation.rows.iter().enumerate().any(|(index, row)| {
                        !row.winner && evaluation.native_winner_indices.contains(&index)
                    })
                });
            if unresolved {
                convention.push_str(" · ▲ unresolved co-optimum");
            }
            convention
        }
        EvaluatorKind::HarmonicGrammar => {
            "cost = Σ(w × violations); lower is better; exact rationals remain exact".to_owned()
        }
        EvaluatorKind::MaxEnt => format!(
            "E = Σ(w × violations); u = ρ exp(−E/T); T = {}; gauge = stored weights; Z ≈ {}; P rounded to 6 decimals; ☞ modal candidate",
            tableau
                .temperature_scalar_or(&document.temperature)
                .canonical(),
            normalizer.map_or_else(
                || "calculated from complete displayed support".to_owned(),
                |value| compact_decimal(value, 9)
            )
        ),
    }
}

#[derive(Debug, Clone)]
struct TableLayout {
    caption_height: f32,
    candidate_width: f32,
    constraint_widths: Vec<f32>,
    metric_widths: Vec<f32>,
    header_height: f32,
    row_heights: Vec<f32>,
    width: f32,
    height: f32,
}

fn constraint_widths(tableau: &Tableau, evaluator: EvaluatorKind) -> Vec<f32> {
    tableau
        .constraints
        .iter()
        .map(|constraint| {
            let name = measure_text(&constraint.name, 12.0, 600) + 16.0;
            let detail = if constraint.definition.is_empty() {
                0.0
            } else {
                measure_text(&constraint.definition, 10.0, 400) + 16.0
            };
            let weight = if evaluator == EvaluatorKind::Ot {
                0.0
            } else {
                constraint
                    .weight
                    .as_ref()
                    .map(|value| measure_text(&format!("w = {}", value.canonical()), 10.0, 400))
                    .unwrap_or(100.0)
                    + 16.0
            };
            name.max(detail).max(weight).clamp(86.0, 184.0)
        })
        .collect()
}

fn table_layout(tableau: &Tableau, document: &ConvalgenDocument) -> TableLayout {
    let evaluator = tableau.evaluator_or(document.evaluator);
    let candidate_measure = tableau
        .candidates
        .iter()
        .map(|candidate| {
            let form = if candidate.form.trim().is_empty() {
                &candidate.name
            } else {
                &candidate.form
            };
            measure_text(form, 12.5, 400) + 70.0
        })
        .chain(std::iter::once(
            measure_text(&tableau.input, 13.0, 500) + 28.0,
        ))
        .fold(184.0_f32, f32::max);
    let candidate_width = candidate_measure.clamp(184.0, 360.0);
    let constraint_widths = constraint_widths(tableau, evaluator);
    let mut metric_widths = metric_headers(evaluator)
        .iter()
        .map(|label| (measure_text(label, 11.5, 600) + 26.0).max(88.0))
        .collect::<Vec<_>>();
    // Computed cells remain read-only, but they still participate in layout.
    // In particular, an exact rational HG cost can be substantially wider
    // than the short `cost` header; sizing only from the header would let the
    // value cross a rule in the exported tableau.
    match evaluator {
        EvaluatorKind::Ot => {}
        EvaluatorKind::HarmonicGrammar => {
            for candidate in &tableau.candidates {
                if let Ok(value) = exact_cost(tableau, candidate) {
                    metric_widths[0] = metric_widths[0].max(measure_text(&value, 11.5, 400) + 28.0);
                }
            }
        }
        EvaluatorKind::MaxEnt => {
            for candidate in &tableau.candidates {
                if let Ok(value) = exact_cost(tableau, candidate) {
                    metric_widths[0] = metric_widths[0].max(measure_text(&value, 10.8, 400) + 28.0);
                }
                let base_mass = candidate.base_mass.canonical();
                metric_widths[1] = metric_widths[1].max(measure_text(&base_mass, 10.8, 400) + 28.0);
            }
        }
    }

    let input_lines = wrap_text(&tableau.input, candidate_width - 18.0, 13.0).len();
    let mut header_lines = input_lines;
    for (constraint, width) in tableau.constraints.iter().zip(&constraint_widths) {
        let mut lines = wrap_text(&constraint.name, *width - 12.0, 12.0).len();
        if !constraint.definition.trim().is_empty() {
            lines += wrap_text(&constraint.definition, *width - 12.0, 10.0).len();
        }
        if evaluator != EvaluatorKind::Ot {
            lines += 1;
        }
        header_lines = header_lines.max(lines);
    }
    let header_height =
        (18.0 + header_lines as f32 * 14.0).max(if evaluator == EvaluatorKind::Ot {
            42.0
        } else {
            56.0
        });
    let row_heights = tableau
        .candidates
        .iter()
        .map(|candidate| {
            let form = if candidate.form.trim().is_empty() {
                &candidate.name
            } else {
                &candidate.form
            };
            let lines = wrap_text(form, candidate_width - 70.0, 12.5).len();
            (14.0 + lines as f32 * 15.0).max(if document.presentation.compact_rows {
                34.0
            } else {
                42.0
            })
        })
        .collect::<Vec<_>>();
    let constraints_total: f32 = constraint_widths.iter().sum();
    let metrics_total: f32 = metric_widths.iter().sum();
    let width = candidate_width
        + BOUNDARY_GAP
        + constraints_total
        + if metric_widths.is_empty() {
            0.0
        } else {
            BOUNDARY_GAP + metrics_total
        };
    // Keep the explanatory line outside the closed grid even at the narrowest
    // valid tableau width. This prevents text from colliding with the top rule.
    let convention = tableau_convention(tableau, document, evaluator, None);
    let convention_lines = wrap_text(&convention, width, 10.0).len();
    let caption_height = 46.0 + convention_lines.saturating_sub(1) as f32 * 12.0;
    let height = caption_height + header_height + row_heights.iter().sum::<f32>();
    TableLayout {
        caption_height,
        candidate_width,
        constraint_widths,
        metric_widths,
        header_height,
        row_heights,
        width,
        height,
    }
}

fn exact_cost(tableau: &Tableau, candidate: &Candidate) -> Result<String, String> {
    let mut total = NumericScalar::integer(0);
    for (index, constraint) in tableau.constraints.iter().enumerate() {
        if !constraint.enabled {
            continue;
        }
        let weight = constraint
            .weight
            .as_ref()
            .ok_or_else(|| format!("weight for {} is unavailable", constraint.name))?;
        let mark = resolved_violation(tableau, candidate, index)?;
        let product = weight
            .checked_mul(&NumericScalar::integer(mark))
            .map_err(|error| error.to_string())?;
        total = total
            .checked_add(&product)
            .map_err(|error| error.to_string())?;
    }
    Ok(total.canonical())
}

fn compact_decimal(value: f64, digits: usize) -> String {
    if !value.is_finite() {
        return "not finite".to_owned();
    }
    let mut result = format!("{value:.digits$}");
    while result.contains('.') && result.ends_with('0') {
        result.pop();
    }
    if result.ends_with('.') {
        result.pop();
    }
    if result == "-0" {
        "0".to_owned()
    } else {
        result
    }
}

fn maxent_masses(
    tableau: &Tableau,
    document: &ConvalgenDocument,
) -> Result<(Vec<f64>, f64), String> {
    let temperature = tableau
        .temperature_scalar_or(&document.temperature)
        .to_f64_center()
        .map_err(|error| error.to_string())?;
    let mut masses = Vec::with_capacity(tableau.candidates.len());
    for candidate in &tableau.candidates {
        let cost = exact_cost(tableau, candidate)?;
        let cost = NumericScalar::parse_editor(&cost)
            .and_then(|value| value.to_f64_center())
            .map_err(|error| error.to_string())?;
        let base = candidate
            .base_mass
            .to_f64_center()
            .map_err(|error| error.to_string())?;
        masses.push(base * (-cost / temperature).exp());
    }
    let normalizer = masses.iter().sum();
    Ok((masses, normalizer))
}

fn reaches(edges: &[(usize, usize)], source: usize, target: usize) -> bool {
    let mut stack = vec![source];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current) {
            continue;
        }
        for (_, next) in edges.iter().filter(|(from, _)| *from == current) {
            if *next == target {
                return true;
            }
            stack.push(*next);
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundaryKind {
    Strict,
    Tied,
    Independent,
}

fn boundary_kind(document: &ConvalgenDocument, tableau: &Tableau, left: usize) -> BoundaryKind {
    if left + 1 >= tableau.constraints.len() {
        return BoundaryKind::Strict;
    }
    if tableau.constraints[left].stratum == tableau.constraints[left + 1].stratum {
        return BoundaryKind::Tied;
    }
    if !document.a_priori_rankings.is_empty()
        && !reaches(&document.a_priori_rankings, left, left + 1)
        && !reaches(&document.a_priori_rankings, left + 1, left)
    {
        return BoundaryKind::Independent;
    }
    BoundaryKind::Strict
}

fn render_tableau(
    svg: &mut String,
    tableau: &Tableau,
    document: &ConvalgenDocument,
    x: f32,
    y: f32,
    label: &str,
) -> Result<f32, String> {
    let layout = table_layout(tableau, document);
    let evaluator = tableau.evaluator_or(document.evaluator);
    let evaluation = PhonologicalEngine::new()
        .evaluate(
            tableau,
            evaluator,
            tableau.temperature_or(&document.temperature),
        )
        .map_err(|problem| problem.to_string())?;
    let (masses, normalizer) = if evaluator == EvaluatorKind::MaxEnt {
        maxent_masses(tableau, document)?
    } else {
        (Vec::new(), 0.0)
    };
    let tableau_id = xml_id("tableau", &tableau.id);
    let top = y + layout.caption_height;
    let body_top = top + layout.header_height;
    let bottom = body_top + layout.row_heights.iter().sum::<f32>();
    let constraints_left = x + layout.candidate_width + BOUNDARY_GAP;
    let constraints_width: f32 = layout.constraint_widths.iter().sum();
    let metrics_left = constraints_left + constraints_width + BOUNDARY_GAP;

    // Publication mode reads these semantic boundaries from the native SVG.
    // They do not alter the unsliced export; they let a fixed-page view repeat
    // the input/header band and candidate column while breaking only between
    // candidate rows and analysis columns.
    let mut publication_column_breaks = vec![constraints_left];
    let mut publication_column_right = constraints_left;
    for width in &layout.constraint_widths {
        publication_column_right += width;
        publication_column_breaks.push(publication_column_right);
    }
    if !layout.metric_widths.is_empty() {
        publication_column_right += BOUNDARY_GAP;
        for width in &layout.metric_widths {
            publication_column_right += width;
            publication_column_breaks.push(publication_column_right);
        }
    }
    let publication_column_breaks = publication_column_breaks
        .iter()
        .map(|value| format!("{value:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut publication_row_breaks = vec![body_top];
    let mut publication_row_bottom = body_top;
    for height in &layout.row_heights {
        publication_row_bottom += height;
        publication_row_breaks.push(publication_row_bottom);
    }
    let publication_row_breaks = publication_row_breaks
        .iter()
        .map(|value| format!("{value:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    group_start(
        svg,
        &tableau_id,
        "tableau",
        &format!(
            r#"data-evaluator="{}" data-width="{:.1}" data-height="{:.1}" data-publication-layout="tableau" data-publication-table-left="{x:.1}" data-publication-header-top="{top:.1}" data-publication-body-top="{body_top:.1}" data-publication-pinned-right="{constraints_left:.1}" data-publication-column-breaks="{publication_column_breaks}" data-publication-row-breaks="{publication_row_breaks}" data-publication-constraint-count="{}""#,
            evaluator.short_label(),
            layout.width,
            layout.height,
            tableau.constraints.len()
        ),
    );
    title_node(
        svg,
        &format!(
            "{}: {} under {}",
            label,
            counted(tableau.candidates.len(), "candidate", "candidates"),
            evaluator.label()
        ),
    );
    text(svg, x, y + 17.0, 15.0, 650, label);
    let convention = tableau_convention(tableau, document, evaluator, Some(normalizer));
    let convention_lines = wrap_text(&convention, layout.width, 10.0);
    text_lines(svg, x, y + 34.0, 12.0, 10.0, 400, &convention_lines);

    let _ = writeln!(
        svg,
        r#"<rect id="{tableau_id}-border" x="{x:.1}" y="{top:.1}" width="{:.1}" height="{:.1}" fill="none" stroke="{LINE}" stroke-width="1"/>"#,
        layout.width,
        bottom - top
    );
    line(
        svg,
        x,
        top + layout.header_height,
        x + layout.width,
        top + layout.header_height,
        1.0,
    );
    line(
        svg,
        x,
        top + layout.header_height + 3.0,
        x + layout.width,
        top + layout.header_height + 3.0,
        1.0,
    );

    group_start(
        svg,
        &format!("{tableau_id}-candidate-header"),
        "candidate-header",
        "",
    );
    title_node(svg, "Input header; rows below are candidates for output");
    let input_lines = wrap_text(&tableau.input, layout.candidate_width - 18.0, 13.0);
    centered_lines(
        svg,
        x + layout.candidate_width / 2.0,
        top + layout.header_height / 2.0,
        15.0,
        13.0,
        550,
        &input_lines,
    );
    group_end(svg);
    line(
        svg,
        x + layout.candidate_width,
        top,
        x + layout.candidate_width,
        bottom,
        1.2,
    );
    line(
        svg,
        x + layout.candidate_width + 3.0,
        top,
        x + layout.candidate_width + 3.0,
        bottom,
        1.2,
    );

    let mut constraint_left = constraints_left;
    for (index, (constraint, width)) in tableau
        .constraints
        .iter()
        .zip(layout.constraint_widths.iter().copied())
        .enumerate()
    {
        let constraint_id = xml_id("constraint-column", &constraint.id);
        group_start(
            svg,
            &format!("{tableau_id}-{constraint_id}"),
            "constraint-column",
            &format!(r#"data-constraint-index="{index}""#),
        );
        title_node(
            svg,
            &format!("Constraint {}: {}", index + 1, constraint.name),
        );
        let mut header_lines = wrap_text(&constraint.name, width - 12.0, 12.0);
        if !constraint.definition.trim().is_empty() {
            header_lines.extend(wrap_text(&constraint.definition, width - 12.0, 10.0));
        }
        if evaluator != EvaluatorKind::Ot {
            header_lines.push(format!(
                "w = {}",
                constraint
                    .weight
                    .as_ref()
                    .map_or_else(|| "unavailable".to_owned(), NumericScalar::canonical)
            ));
        }
        centered_lines(
            svg,
            constraint_left + width / 2.0,
            top + layout.header_height / 2.0,
            13.0,
            if header_lines.len() > 2 { 10.0 } else { 12.0 },
            550,
            &header_lines,
        );
        group_end(svg);
        let boundary_x = constraint_left + width;
        match boundary_kind(document, tableau, index) {
            BoundaryKind::Strict => strict_boundary_line(svg, boundary_x, top, bottom),
            BoundaryKind::Tied => dotted_line(svg, boundary_x, top, bottom),
            BoundaryKind::Independent => jagged_line(svg, boundary_x, top, bottom),
        }
        constraint_left += width;
    }

    if !layout.metric_widths.is_empty() {
        line(
            svg,
            constraints_left + constraints_width,
            top,
            constraints_left + constraints_width,
            bottom,
            1.2,
        );
        line(
            svg,
            constraints_left + constraints_width + 3.0,
            top,
            constraints_left + constraints_width + 3.0,
            bottom,
            1.2,
        );
        let mut left = metrics_left;
        for (index, (label, width)) in metric_headers(evaluator)
            .iter()
            .zip(layout.metric_widths.iter().copied())
            .enumerate()
        {
            group_start(
                svg,
                &format!("{tableau_id}-metric-column-{index}"),
                "metric-column",
                &format!(r#"data-metric="{}""#, xml(label)),
            );
            title_node(
                svg,
                &format!("Computed metric {label}; cells are read-only"),
            );
            centered(
                svg,
                left + width / 2.0,
                top + layout.header_height / 2.0,
                11.5,
                650,
                label,
            );
            group_end(svg);
            left += width;
            if index + 1 < layout.metric_widths.len() {
                line(svg, left, top, left, bottom, 1.0);
            }
        }
    }

    let mut row_top = top + layout.header_height;
    for (row_index, candidate) in tableau.candidates.iter().enumerate() {
        let row_height = layout.row_heights[row_index];
        let result = &evaluation.rows[row_index];
        let candidate_id = xml_id("candidate-row", &candidate.id);
        group_start(
            svg,
            &format!("{tableau_id}-{candidate_id}"),
            "candidate-row",
            &format!(
                r#"data-candidate-index="{row_index}" data-winner="{}""#,
                result.winner
            ),
        );
        let native_co_winner = evaluation.native_winner_indices.contains(&row_index);
        title_node(
            svg,
            &format!(
                "Candidate {}: {}; {}",
                row_index + 1,
                candidate.name,
                if result.winner {
                    "selected winner"
                } else if native_co_winner {
                    "native co-optimum not selected by the declared tie policy"
                } else {
                    "not selected"
                }
            ),
        );
        if evaluator == EvaluatorKind::Ot
            && let Some(fatal) = result.fatal_constraint
        {
            let fatal_stratum = tableau.constraints[fatal].stratum;
            let mut left = constraints_left;
            for (constraint_index, width) in layout.constraint_widths.iter().copied().enumerate() {
                if tableau.constraints[constraint_index].stratum > fatal_stratum {
                    rect(svg, left, row_top, width, row_height, SOFT);
                }
                left += width;
            }
        }
        let marker = if result.winner {
            "☞"
        } else if native_co_winner {
            "▲"
        } else {
            ""
        };
        centered(svg, x + 16.0, row_top + row_height / 2.0, 14.0, 600, marker);
        centered(
            svg,
            x + 42.0,
            row_top + row_height / 2.0,
            11.5,
            400,
            &candidate_label(row_index),
        );
        let form = if candidate.form.trim().is_empty() {
            &candidate.name
        } else {
            &candidate.form
        };
        let form_lines = wrap_text(form, layout.candidate_width - 70.0, 12.5);
        let form_start =
            row_top + row_height / 2.0 - form_lines.len().saturating_sub(1) as f32 * 7.5 + 4.0;
        text_lines(svg, x + 58.0, form_start, 15.0, 12.5, 400, &form_lines);

        let mut left = constraints_left;
        for (constraint_index, width) in layout.constraint_widths.iter().copied().enumerate() {
            let mark = resolved_violation(tableau, candidate, constraint_index)?;
            let fatal = result.fatal_constraint == Some(constraint_index);
            let mark_text = match evaluator {
                EvaluatorKind::Ot if mark == 0 => String::new(),
                EvaluatorKind::Ot if mark <= 6 => "*".repeat(mark as usize),
                EvaluatorKind::Ot => format!("{mark}×*"),
                EvaluatorKind::HarmonicGrammar | EvaluatorKind::MaxEnt => mark.to_string(),
            };
            group_start(
                svg,
                &format!("{tableau_id}-{candidate_id}-constraint-{constraint_index}"),
                "violation-cell",
                &format!(r#"data-constraint-index="{constraint_index}""#),
            );
            centered(
                svg,
                left + width / 2.0,
                row_top + row_height / 2.0,
                12.5,
                if fatal { 700 } else { 400 },
                &if fatal {
                    format!("{mark_text}!")
                } else {
                    mark_text
                },
            );
            group_end(svg);
            left += width;
        }

        if evaluator == EvaluatorKind::HarmonicGrammar {
            let value = exact_cost(tableau, candidate)?;
            centered(
                svg,
                metrics_left + layout.metric_widths[0] / 2.0,
                row_top + row_height / 2.0,
                11.5,
                if result.winner { 700 } else { 400 },
                &value,
            );
        } else if evaluator == EvaluatorKind::MaxEnt {
            let values = [
                exact_cost(tableau, candidate)?,
                candidate.base_mass.canonical(),
                format!("≈{}", compact_decimal(masses[row_index], 8)),
                format!("≈{}", compact_decimal(result.probability.unwrap_or(0.0), 6)),
            ];
            let mut left = metrics_left;
            for (index, (value, width)) in values
                .iter()
                .zip(layout.metric_widths.iter().copied())
                .enumerate()
            {
                group_start(
                    svg,
                    &format!("{tableau_id}-{candidate_id}-metric-{index}"),
                    "metric-cell",
                    r#"data-derived="true""#,
                );
                centered(
                    svg,
                    left + width / 2.0,
                    row_top + row_height / 2.0,
                    10.8,
                    if index == 3 && result.winner {
                        700
                    } else {
                        400
                    },
                    value,
                );
                group_end(svg);
                left += width;
            }
        }
        line(
            svg,
            x,
            row_top + row_height,
            x + layout.width,
            row_top + row_height,
            0.7,
        );
        group_end(svg);
        row_top += row_height;
    }
    group_end(svg);
    Ok(layout.height)
}

fn metadata_lines(document: &ConvalgenDocument, width: f32) -> Vec<(f32, u16, String)> {
    let mut lines = Vec::new();
    if document.presentation.show_title && !document.title.trim().is_empty() {
        lines.extend(
            wrap_text(&document.title, width, 18.0)
                .into_iter()
                .map(|line| (18.0, 700, line)),
        );
    }
    if document.presentation.show_author && !document.author.trim().is_empty() {
        lines.extend(
            wrap_text(&document.author, width, 11.0)
                .into_iter()
                .map(|line| (11.0, 400, line)),
        );
    }
    if document.presentation.show_legend {
        lines.extend(
            wrap_text(
                &format!(
                    "{} · candidates are competing rows; only a selected winner is an output",
                    document.evaluator.label()
                ),
                width,
                10.0,
            )
            .into_iter()
            .map(|line| (10.0, 400, line)),
        );
        if document.evaluator == EvaluatorKind::Ot {
            lines.extend(
                wrap_text(
                    "boundaries: solid = strict dominance · dotted = tied stratum · zigzag = independent partial order",
                    width,
                    10.0,
                )
                .into_iter()
                .map(|line| (10.0, 400, line)),
            );
        }
    }
    lines
}

fn metadata_height(lines: &[(f32, u16, String)]) -> f32 {
    if lines.is_empty() {
        0.0
    } else {
        lines.iter().map(|(size, _, _)| size + 7.0).sum::<f32>() + 5.0
    }
}

fn render_metadata(svg: &mut String, x: f32, y: f32, lines: &[(f32, u16, String)]) -> f32 {
    let mut baseline = y;
    for (size, weight, value) in lines {
        baseline += size + 3.0;
        text(svg, x, baseline, *size, *weight, value);
        baseline += 4.0;
    }
    baseline - y
}

fn first_order_svg(document: &ConvalgenDocument) -> Result<String, String> {
    let layout = table_layout(&document.source, document);
    let title_width = if document.presentation.show_title {
        measure_text(&document.title, 18.0, 700)
    } else {
        0.0
    };
    let content_width = layout.width.max(title_width.min(1200.0));
    let metadata = metadata_lines(document, content_width);
    let metadata_height = metadata_height(&metadata);
    let width = content_width + PADDING * 2.0;
    let height = metadata_height + layout.height + PADDING * 2.0;
    let mut svg = svg_start(
        width,
        height,
        if document.title.trim().is_empty() {
            "Constraint tableau"
        } else {
            &document.title
        },
        &format!(
            "Content-cropped {} tableau with {} and {}.",
            document.source.evaluator_or(document.evaluator).label(),
            counted(document.source.candidates.len(), "candidate", "candidates"),
            counted(
                document.source.constraints.len(),
                "constraint",
                "constraints"
            )
        ),
    );
    group_start(&mut svg, &xml_id("document", &document.id), "document", "");
    let used = render_metadata(&mut svg, PADDING, PADDING, &metadata);
    let label = if document.source.name.trim().is_empty() {
        "TABLEAU"
    } else {
        &document.source.name
    };
    render_tableau(
        &mut svg,
        &document.source,
        document,
        PADDING,
        PADDING + used,
        label,
    )?;
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    Ok(svg)
}

fn answer_text(answer: &[Vec<String>]) -> String {
    if answer.is_empty() {
        return "unavailable".to_owned();
    }
    answer
        .iter()
        .map(|lane| {
            if lane.is_empty() {
                "∅".to_owned()
            } else {
                format!("{{{}}}", lane.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn comparison_symbol(status: ComparisonStatus) -> Option<&'static str> {
    match status {
        ComparisonStatus::Preserved => Some("="),
        ComparisonStatus::Discrepant => Some("≠"),
        // The dissertation and the visual contract make the words themselves
        // the refusal mark.  Avoid adding an uncommon glyph that can turn into
        // tofu in an otherwise portable export.
        ComparisonStatus::NotEvaluated => None,
    }
}

fn formation_and_admission(result: &SecondOrderResult) -> (&'static str, &'static str) {
    let Some(refusal) = &result.refusal else {
        return ("formed", "admitted");
    };
    match refusal.stage {
        crate::engine::ContractStage::Formation => ("refused", "not reached"),
        crate::engine::ContractStage::Admission => ("formed", "refused"),
        crate::engine::ContractStage::Evaluation | crate::engine::ContractStage::Certification => {
            ("formed", "admitted")
        }
    }
}

fn contract_entries(
    document: &ConvalgenDocument,
    result: &SecondOrderResult,
) -> Vec<(String, String)> {
    let (formation, admission) = formation_and_admission(result);
    let comparison = match document.second_order.comparison_mode {
        crate::model::ComparisonMode::Exact => "exact".to_owned(),
        crate::model::ComparisonMode::Approximate => format!(
            "approximate; tolerance {}",
            document.second_order.tolerance.canonical()
        ),
        crate::model::ComparisonMode::Grid => format!(
            "grid-based; step {}; tolerance {}",
            document.second_order.grid_step.canonical(),
            document.second_order.tolerance.canonical()
        ),
    };
    let mut entries = vec![
        (
            "query".to_owned(),
            format!(
                "Q1 [{}] {}; answer sort: {}",
                query_code(document.second_order.query),
                document.second_order.query.label(),
                document.second_order.answer_sort
            ),
        ),
        ("scope".to_owned(), document.second_order.scope.clone()),
        (
            "source".to_owned(),
            format!(
                "{} · layer {}",
                document.source.id, document.second_order.source_layer
            ),
        ),
        (
            "target".to_owned(),
            format!(
                "{} · layer {}",
                document.target.id, document.second_order.target_layer
            ),
        ),
        (
            "transformation".to_owned(),
            document.second_order.transformation.clone(),
        ),
        (
            "answer transport".to_owned(),
            document.second_order.transport.clone(),
        ),
        (
            "layer transport".to_owned(),
            document.second_order.layer_transport.clone(),
        ),
        (
            "formation / admission".to_owned(),
            format!("{formation} / {admission}"),
        ),
        ("comparison".to_owned(), comparison),
        (
            "source answer".to_owned(),
            answer_text(&result.source_answer),
        ),
        (
            "transported source answer".to_owned(),
            answer_text(&result.transported_source_answer),
        ),
        (
            "target answer".to_owned(),
            answer_text(&result.target_answer),
        ),
    ];
    if document.evaluator == EvaluatorKind::MaxEnt {
        entries.push((
            "normalizers".to_owned(),
            format!(
                "source {}; target {}; policy {}",
                result.source_normalizer.as_deref().unwrap_or("unavailable"),
                result.target_normalizer.as_deref().unwrap_or("unavailable"),
                document.second_order.normalizer_policy.label()
            ),
        ));
    }
    let outcome = if let Some(refusal) = &result.refusal {
        format!(
            "{} [{}:{}] {} Remedy: {}",
            refusal.code,
            refusal.stage.label(),
            refusal.coordinate,
            refusal.message,
            refusal.remedy
        )
    } else if !result.discrepancies.is_empty() {
        result
            .discrepancies
            .iter()
            .map(|item| {
                format!(
                    "{}: source {} · target {} · {}",
                    item.coordinate, item.source, item.target, item.difference
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    } else if let Some(certificate) = &result.certificate {
        format!(
            "{} Evidence: {}",
            certificate.statement,
            certificate.evidence.join("; ")
        )
    } else {
        "no certificate, discrepancy, or refusal was returned".to_owned()
    };
    entries.push(("result".to_owned(), outcome));
    entries
}

fn contract_key_width(entries: &[(String, String)], width: f32) -> f32 {
    entries
        .iter()
        .map(|(key, _)| measure_text(&key.to_uppercase(), 10.0, 700) + 20.0)
        .fold(150.0_f32, f32::max)
        .min(width * 0.42)
}

fn contract_height(entries: &[(String, String)], width: f32) -> f32 {
    let key_width = contract_key_width(entries, width);
    entries
        .iter()
        .map(|(_key, value)| {
            let lines = wrap_text(value, width - key_width - 18.0, 10.5).len();
            (lines as f32 * 13.0 + 8.0).max(22.0)
        })
        .sum::<f32>()
        + 2.0
}

fn render_contract(
    svg: &mut String,
    x: f32,
    y: f32,
    width: f32,
    entries: &[(String, String)],
) -> f32 {
    group_start(svg, "second-order-contract", "second-order-contract", "");
    title_node(
        svg,
        "Declared Second-Order comparison contract and independently calculated answers",
    );
    let key_width = contract_key_width(entries, width);
    let mut top = y;
    for (index, (key, value)) in entries.iter().enumerate() {
        let lines = wrap_text(value, width - key_width - 18.0, 10.5);
        let row_height = (lines.len() as f32 * 13.0 + 8.0).max(22.0);
        if index % 2 == 1 {
            rect(svg, x, top, width, row_height, SOFT);
        }
        text(svg, x + 7.0, top + 15.0, 10.0, 700, &key.to_uppercase());
        text_lines(svg, x + key_width, top + 15.0, 13.0, 10.5, 400, &lines);
        line(svg, x, top + row_height, x + width, top + row_height, 0.5);
        top += row_height;
    }
    let _ = writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{:.1}" fill="none" stroke="{LINE}" stroke-width="1"/>"#,
        top - y
    );
    group_end(svg);
    top - y
}

fn comparison_lanes(document: &ConvalgenDocument) -> Vec<(String, SecondOrderResult)> {
    if document.serial.moves.is_empty() && document.target_serial.moves.is_empty() {
        return vec![(
            document.second_order.response_domain.label().to_owned(),
            PhonologicalEngine::new().compare(document),
        )];
    }
    [ResponseDomain::Terminal, ResponseDomain::Trajectory]
        .into_iter()
        .map(|domain| {
            let mut copy = document.clone();
            copy.second_order.response_domain = domain;
            (
                domain.label().to_owned(),
                PhonologicalEngine::new().compare(&copy),
            )
        })
        .collect()
}

fn render_status_lanes(
    svg: &mut String,
    x: f32,
    y: f32,
    width: f32,
    lanes: &[(String, SecondOrderResult)],
) -> f32 {
    let mut top = y;
    for (index, (label, result)) in lanes.iter().enumerate() {
        let id = format!("query-lane-{index}");
        group_start(
            svg,
            &id,
            "query-lane",
            &format!(r#"data-status="{}""#, result.status.label()),
        );
        title_node(svg, &format!("{label}: {}", result.status.label()));
        let _ = writeln!(
            svg,
            r#"<rect x="{x:.1}" y="{top:.1}" width="{width:.1}" height="34" fill="{}" stroke="{LINE}" stroke-width="1"/>"#,
            if index % 2 == 0 { WHITE } else { SOFT }
        );
        text(svg, x + 10.0, top + 22.0, 11.0, 650, label);
        let status_text = comparison_symbol(result.status).map_or_else(
            || result.status.label().to_owned(),
            |symbol| format!("{symbol}  {}", result.status.label()),
        );
        right_text(svg, x + width - 10.0, top + 22.0, 12.0, 700, &status_text);
        group_end(svg);
        top += 40.0;
    }
    top - y
}

fn delta_sidecar_lines(document: &ConvalgenDocument, result: &SecondOrderResult) -> Vec<String> {
    let mut lines = vec![format!("layout: {}", document.second_order.layout.label())];
    for (index, source) in document.source.constraints.iter().enumerate() {
        let target = document
            .target
            .constraints
            .get(index)
            .map(|item| item.name.as_str())
            .unwrap_or("absent");
        if source.name != target {
            lines.push(format!(
                "constraint {}: {} → {target}",
                index + 1,
                source.name
            ));
        }
    }
    for (index, source) in document.source.candidates.iter().enumerate() {
        if let Some(target) = document.target.candidates.get(index)
            && (source.form != target.form || source.violations != target.violations)
        {
            lines.push(format!(
                "candidate {}: {} {:?} → {} {:?}",
                index + 1,
                source.form,
                source.violations,
                target.form,
                target.violations
            ));
        }
    }
    if lines.len() == 1 {
        lines.push("no first-order ledger coordinate changed".to_owned());
    }
    lines.push(format!("comparison: {}", result.status.label()));
    lines
}

fn render_sidecar(svg: &mut String, x: f32, y: f32, width: f32, lines: &[String]) -> f32 {
    let wrapped = lines
        .iter()
        .flat_map(|line| wrap_text(line, width - 20.0, 10.0))
        .collect::<Vec<_>>();
    let height = 34.0 + wrapped.len() as f32 * 13.0;
    group_start(svg, "delta-sidecar", "delta-sidecar", "");
    title_node(
        svg,
        "Declared maps and changed coordinates; source and target tableaux remain complete",
    );
    let _ = writeln!(
        svg,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}" fill="{SOFT}" stroke="{LINE}" stroke-width="1"/>"#
    );
    text(svg, x + 10.0, y + 20.0, 11.0, 700, "DELTA SIDECAR");
    text_lines(svg, x + 10.0, y + 39.0, 13.0, 10.0, 400, &wrapped);
    group_end(svg);
    height
}

fn second_order_svg(document: &ConvalgenDocument) -> Result<String, String> {
    let source_layout = table_layout(&document.source, document);
    let target_layout = table_layout(&document.target, document);
    let result = PhonologicalEngine::new().compare(document);
    let lanes = comparison_lanes(document);
    let geometry_width = match document.second_order.layout {
        SecondOrderLayout::ExpandedPaired
            if source_layout.width + target_layout.width + 28.0 <= 1500.0 =>
        {
            source_layout.width + target_layout.width + 28.0
        }
        SecondOrderLayout::DeltaSidecar => source_layout
            .width
            .max(target_layout.width)
            .max(source_layout.width + 28.0 + 330.0),
        SecondOrderLayout::Overlay => source_layout.width.max(target_layout.width),
        SecondOrderLayout::ExpandedPaired => source_layout.width.max(target_layout.width),
    };
    let content_width = geometry_width.max(760.0);
    let metadata = metadata_lines(document, content_width);
    let metadata_height = metadata_height(&metadata);
    let lane_height = lanes.len() as f32 * 40.0;
    let entries = contract_entries(document, &result);
    let contract_height = contract_height(&entries, content_width);
    let geometry_height = match document.second_order.layout {
        SecondOrderLayout::ExpandedPaired
            if source_layout.width + target_layout.width + 28.0 <= 1500.0 =>
        {
            source_layout.height.max(target_layout.height)
        }
        SecondOrderLayout::DeltaSidecar => {
            source_layout.height.max(180.0) + 24.0 + target_layout.height
        }
        SecondOrderLayout::Overlay => source_layout.height + 34.0 + target_layout.height,
        SecondOrderLayout::ExpandedPaired => source_layout.height + 24.0 + target_layout.height,
    };
    let height =
        PADDING * 2.0 + metadata_height + lane_height + contract_height + 28.0 + geometry_height;
    let width = content_width + PADDING * 2.0;
    let mut svg = svg_start(
        width,
        height,
        &format!("Second-Order Tableau — {}", document.title),
        &format!(
            "{} layout; comparison status {}. Source and target answers are calculated independently before transport comparison.",
            document.second_order.layout.label(),
            result.status.label()
        ),
    );
    group_start(
        &mut svg,
        &xml_id("document", &document.id),
        "document second-order-tableau",
        &format!(r#"data-layout="{}""#, document.second_order.layout.label()),
    );
    let mut y = PADDING;
    y += render_metadata(&mut svg, PADDING, y, &metadata);
    y += render_status_lanes(&mut svg, PADDING, y, content_width, &lanes);
    y += render_contract(&mut svg, PADDING, y, content_width, &entries);
    y += 18.0;
    group_start(
        &mut svg,
        "second-order-evidence",
        "second-order-evidence",
        "",
    );
    match document.second_order.layout {
        SecondOrderLayout::ExpandedPaired
            if source_layout.width + target_layout.width + 28.0 <= 1500.0 =>
        {
            render_tableau(&mut svg, &document.source, document, PADDING, y, "SOURCE")?;
            render_tableau(
                &mut svg,
                &document.target,
                document,
                PADDING + source_layout.width + 28.0,
                y,
                "TARGET",
            )?;
            group_start(&mut svg, "answer-transport", "transport", "");
            title_node(
                &mut svg,
                "Declared answer transport; it does not generate the target answer",
            );
            centered(
                &mut svg,
                PADDING + source_layout.width + 14.0,
                y + 19.0,
                14.0,
                700,
                "→",
            );
            group_end(&mut svg);
        }
        SecondOrderLayout::DeltaSidecar => {
            render_tableau(&mut svg, &document.source, document, PADDING, y, "SOURCE")?;
            let sidecar = delta_sidecar_lines(document, &result);
            render_sidecar(
                &mut svg,
                PADDING + source_layout.width + 28.0,
                y + source_layout.caption_height,
                330.0,
                &sidecar,
            );
            y += source_layout.height + 24.0;
            render_tableau(&mut svg, &document.target, document, PADDING, y, "TARGET")?;
        }
        SecondOrderLayout::Overlay => {
            render_tableau(
                &mut svg,
                &document.source,
                document,
                PADDING,
                y,
                "OVERLAY BASE · SOURCE",
            )?;
            y += source_layout.height;
            group_start(&mut svg, "answer-transport", "transport", "");
            title_node(
                &mut svg,
                "Target evidence follows without covering the source ledger",
            );
            text(
                &mut svg,
                PADDING,
                y + 19.0,
                10.5,
                650,
                &format!("OVERLAY DELTA ↓  {}", document.second_order.transport),
            );
            group_end(&mut svg);
            y += 34.0;
            render_tableau(
                &mut svg,
                &document.target,
                document,
                PADDING,
                y,
                "OVERLAY TARGET LAYER",
            )?;
        }
        SecondOrderLayout::ExpandedPaired => {
            render_tableau(&mut svg, &document.source, document, PADDING, y, "SOURCE")?;
            y += source_layout.height;
            group_start(&mut svg, "answer-transport", "transport", "");
            title_node(
                &mut svg,
                "Declared answer transport; target response remains independent",
            );
            text(
                &mut svg,
                PADDING,
                y + 17.0,
                10.5,
                650,
                &format!("TRANSPORT ↓  {}", document.second_order.transport),
            );
            group_end(&mut svg);
            y += 24.0;
            render_tableau(&mut svg, &document.target, document, PADDING, y, "TARGET")?;
        }
    }
    group_end(&mut svg);
    if result.status == ComparisonStatus::Discrepant {
        group_start(&mut svg, "selected-witness", "witness", "");
        title_node(
            &mut svg,
            "The complete discrepancy record is printed in the contract; this group identifies the witness semantics",
        );
        group_end(&mut svg);
    }
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    Ok(svg)
}

pub fn tableau_svg(document: &ConvalgenDocument, second_order: bool) -> Result<String, String> {
    if second_order {
        second_order_svg(document)
    } else {
        first_order_svg(document)
    }
}

fn serial_local_tableau(document: &ConvalgenDocument, form: &str, stage: usize) -> Tableau {
    let moves = document
        .serial
        .moves
        .iter()
        .filter(|movement| movement.from == form)
        .collect::<Vec<_>>();
    Tableau {
        id: format!("serial-stage-{stage}"),
        name: format!("Stage {stage}"),
        input: form.to_owned(),
        constraints: document.source.constraints.clone(),
        candidates: moves
            .iter()
            .enumerate()
            .map(|(index, movement)| Candidate {
                id: format!("serial-stage-{stage}-candidate-{index}"),
                name: movement.to.clone(),
                form: movement.to.clone(),
                violations: movement.violations.clone(),
                base_mass: NumericScalar::integer(1),
                notes: movement.operation.clone(),
                observed_frequency: NumericScalar::integer(0),
                structured: None,
            })
            .collect(),
        tie_policy: document.source.tie_policy.clone(),
        notes: String::new(),
        evaluator: Some(document.evaluator),
        temperature: Some(document.temperature.clone()),
        missing_dependencies: Vec::new(),
        expected_winners: Vec::new(),
        source_locator: document.source.source_locator.clone(),
    }
}

fn serial_panel_count(result: &SerialResult) -> usize {
    if matches!(
        result.stopped.as_str(),
        "refused: cycle detected" | "refused: declared step limit reached"
    ) {
        // The last path item is the result of the final displayed transition,
        // not a newly evaluated stage.  Rendering it as another local tableau
        // would manufacture a selection that the engine never performed.
        result.path.len().saturating_sub(1)
    } else {
        // Convergence needs its final identity panel; formation failures need
        // the current panel so the missing set or co-winner remains visible.
        result.path.len()
    }
}

fn serial_refusal_svg(document: &ConvalgenDocument, message: &str) -> String {
    let width = 760.0;
    let lines = wrap_text(message, width - PADDING * 4.0, 11.0);
    let height = 150.0 + lines.len() as f32 * 15.0;
    let mut svg = svg_start(
        width,
        height,
        &format!("Serial derivation — {}", document.title),
        "Structured serial refusal. No terminal output is invented.",
    );
    group_start(
        &mut svg,
        "serial-derivation",
        "serial-derivation",
        r#"data-status="not-evaluated""#,
    );
    text(&mut svg, PADDING, 42.0, 18.0, 700, &document.title);
    text(&mut svg, PADDING, 72.0, 13.0, 700, "NOT EVALUATED");
    group_start(&mut svg, "serial-refusal", "refusal", "");
    title_node(&mut svg, "Serial formation or admission refusal");
    text_lines(&mut svg, PADDING, 98.0, 15.0, 11.0, 400, &lines);
    group_end(&mut svg);
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    svg
}

pub fn serial_svg(document: &ConvalgenDocument) -> Result<String, String> {
    let engine = PhonologicalEngine::new();
    let result = match engine.serial(
        &document.source,
        &document.serial,
        document.evaluator,
        document
            .temperature
            .to_f64_center()
            .map_err(|error| error.to_string())?,
    ) {
        Ok(result) => result,
        Err(problem) => return Ok(serial_refusal_svg(document, &problem.to_string())),
    };
    let panels = result
        .path
        .iter()
        .take(serial_panel_count(&result))
        .enumerate()
        .map(|(stage, form)| serial_local_tableau(document, form, stage))
        .collect::<Vec<_>>();
    let panel_layouts = panels
        .iter()
        .map(|panel| table_layout(panel, document))
        .collect::<Vec<_>>();
    let introduction = format!(
        "{} · GEN1 local selection · maximum {} steps · every panel lists candidates, not outputs",
        document.evaluator.label(),
        document.serial.maximum_steps
    );
    let stage_footers = panels
        .iter()
        .enumerate()
        .map(|(stage, panel)| {
            if panel.candidates.is_empty() {
                None
            } else {
                let selected = result
                    .path
                    .get(stage + 1)
                    .map(String::as_str)
                    .unwrap_or(&panel.input);
                let operation = result
                    .operations
                    .get(stage)
                    .map(String::as_str)
                    .unwrap_or("identity / stopping check");
                Some(format!(
                    "selected stage winner: {selected} · operation: {operation}"
                ))
            }
        })
        .collect::<Vec<_>>();
    let content_width = panel_layouts
        .iter()
        .map(|layout| layout.width)
        .chain(std::iter::once(
            measure_text(&document.title, 18.0, 700) + 8.0,
        ))
        .chain(std::iter::once(
            measure_text(&introduction, 10.5, 400) + 8.0,
        ))
        .chain(stage_footers.iter().filter_map(|value| {
            value
                .as_ref()
                .map(|text| measure_text(text, 10.5, 650) + 8.0)
        }))
        .chain(std::iter::once(
            measure_text(&result.stopped, 11.0, 400) + 20.0,
        ))
        .fold(620.0_f32, f32::max);
    let stage_extras = panels
        .iter()
        .zip(&stage_footers)
        .map(|(panel, footer)| {
            if panel.candidates.is_empty() {
                44.0
            } else {
                let lines = footer
                    .as_ref()
                    .map(|text| wrap_text(text, content_width, 10.5).len())
                    .unwrap_or(1);
                29.0 + lines as f32 * 13.0
            }
        })
        .collect::<Vec<_>>();
    let panels_height = panels
        .iter()
        .zip(&panel_layouts)
        .zip(&stage_extras)
        .map(|((panel, layout), extra)| {
            if panel.candidates.is_empty() {
                *extra
            } else {
                layout.height + *extra
            }
        })
        .sum::<f32>();
    let height = 82.0 + panels_height + 48.0 + PADDING;
    let width = content_width + PADDING * 2.0;
    let mut svg = svg_start(
        width,
        height,
        &format!("Serial derivation — {}", document.title),
        &format!(
            "{}-stage serial record with local candidate sets and stopping witness {}.",
            panels.len(),
            result.stopped
        ),
    );
    group_start(
        &mut svg,
        "serial-derivation",
        "serial-derivation",
        &format!(r#"data-formed="{}""#, result.formed),
    );
    text(&mut svg, PADDING, 38.0, 18.0, 700, &document.title);
    text(&mut svg, PADDING, 62.0, 10.5, 400, &introduction);
    let mut y = 82.0;
    for (stage, panel) in panels.iter().enumerate() {
        group_start(
            &mut svg,
            &format!("serial-stage-{stage}"),
            "serial-stage",
            &format!(r#"data-stage="{stage}""#),
        );
        if panel.candidates.is_empty() {
            text(
                &mut svg,
                PADDING,
                y + 20.0,
                11.0,
                700,
                &format!(
                    "STAGE {stage} · input {} · no local candidate set",
                    panel.input
                ),
            );
            y += 44.0;
        } else {
            render_tableau(
                &mut svg,
                panel,
                document,
                PADDING,
                y,
                &format!("STAGE {stage} · INPUT {}", panel.input),
            )?;
            y += panel_layouts[stage].height;
            let footer = stage_footers[stage]
                .as_deref()
                .unwrap_or("selected stage winner unavailable");
            let footer_lines = wrap_text(footer, content_width, 10.5);
            text_lines(&mut svg, PADDING, y + 17.0, 13.0, 10.5, 650, &footer_lines);
            text(
                &mut svg,
                PADDING,
                y + 34.0 + footer_lines.len().saturating_sub(1) as f32 * 13.0,
                13.0,
                700,
                "↓",
            );
            y += stage_extras[stage];
        }
        group_end(&mut svg);
    }
    group_start(&mut svg, "serial-stopping-witness", "stopping-witness", "");
    let _ = writeln!(
        svg,
        r#"<rect x="{PADDING:.1}" y="{y:.1}" width="{content_width:.1}" height="48" fill="{SOFT}" stroke="{LINE}" stroke-width="1"/>"#
    );
    text(
        &mut svg,
        PADDING + 10.0,
        y + 19.0,
        10.0,
        700,
        "STOPPING WITNESS",
    );
    text(
        &mut svg,
        PADDING + 10.0,
        y + 37.0,
        11.0,
        400,
        &result.stopped,
    );
    group_end(&mut svg);
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    Ok(svg)
}

pub fn q_calculus_svg(document: &ConvalgenDocument) -> Result<String, String> {
    let engine = PhonologicalEngine::new();
    let temperature = document
        .temperature
        .to_f64_center()
        .map_err(|error| error.to_string())?;
    let result = engine.q_clone_audit(
        &document.source,
        document.clone_constraint,
        &document.a_priori_rankings,
        document.evaluator,
        temperature,
    );
    let constraint = document
        .source
        .constraints
        .get(document.clone_constraint)
        .map(|item| item.name.as_str())
        .unwrap_or("undeclared constraint");
    let mut steps = vec![
        format!(
            "REGISTER source carrier [{}] with {} and {}.",
            document.source.id,
            counted(document.source.candidates.len(), "candidate", "candidates"),
            counted(
                document.source.constraints.len(),
                "constraint",
                "constraints"
            )
        ),
        format!(
            "DECLARE transformation CLONE({constraint}); the cloned column preserves every registered violation mark."
        ),
        format!(
            "REGISTER query root Q = {}; answer sort = {}.",
            document.second_order.query.label(),
            document.second_order.answer_sort
        ),
    ];
    let (status, terminal, refusal) = match result {
        Ok(audit) => {
            steps.push(format!(
                "ENUMERATE source ranking space: denominator {}; {} distinct answer classes.",
                audit.before.total_rankings,
                audit.before.winner_counts.len()
            ));
            steps.push(format!(
                "APPLY CLONE and enumerate target ranking space: denominator {}; {} distinct answer classes.",
                audit.after.total_rankings,
                audit.after.winner_counts.len()
            ));
            steps.push(format!(
                "COMPARE support = {}; ranking shares = {}; certificate contains {} answer-coordinate shifts.",
                if audit.support_conservative { "preserved" } else { "changed" },
                if audit.shares_conservative { "preserved" } else { "changed" },
                audit.shifts.len()
            ));
            (
                if audit.support_conservative && audit.shares_conservative {
                    "STRUCTURAL AND MASS CONSERVATIVE"
                } else if audit.support_conservative {
                    "STRUCTURAL CONSERVATIVE; MASS NONCONSERVATIVE"
                } else {
                    "STRUCTURAL NONCONSERVATIVE"
                },
                format!(
                    "QResult<support={}, shares={}> over exact finite ranking counts",
                    audit.support_conservative, audit.shares_conservative
                ),
                None,
            )
        }
        Err(problem) => {
            steps.push(format!(
                "STOP at {} [{}:{}]; failed premise remains visible.",
                problem.code, problem.stage, problem.coordinate
            ));
            (
                "NOT EVALUATED",
                "QResult<refusal>".to_owned(),
                Some(format!("{} Remedy: {}", problem.message, problem.remedy)),
            )
        }
    };
    let content_width = 820.0;
    let wrapped_steps = steps
        .iter()
        .map(|step| wrap_text(step, content_width - 66.0, 11.0))
        .collect::<Vec<_>>();
    let derivation_height = wrapped_steps
        .iter()
        .map(|lines| 22.0 + lines.len() as f32 * 15.0)
        .sum::<f32>();
    let refusal_height = refusal
        .as_ref()
        .map(|value| wrap_text(value, content_width - 20.0, 10.5).len() as f32 * 14.0 + 28.0)
        .unwrap_or(0.0);
    let width = content_width + PADDING * 2.0;
    let height = 116.0 + derivation_height + refusal_height + 74.0;
    let mut svg = svg_start(
        width,
        height,
        &format!("Q-Calculus derivation — {}", document.title),
        &format!("Numbered typed Q-Calculus derivation ending in {status}."),
    );
    group_start(
        &mut svg,
        "q-calculus-derivation",
        "q-calculus-derivation",
        &format!(r#"data-tableau-ref="{}""#, xml(&document.source.id)),
    );
    text(&mut svg, PADDING, 38.0, 18.0, 700, "Q-CALCULUS DERIVATION");
    text(
        &mut svg,
        PADDING,
        62.0,
        10.5,
        400,
        "PhonoScript GUI visual convention; this is not an ordinary winner tableau",
    );
    let mut y = 82.0;
    for (index, lines) in wrapped_steps.iter().enumerate() {
        let step_height = 22.0 + lines.len() as f32 * 15.0;
        group_start(
            &mut svg,
            &format!("q-step-{}", index + 1),
            "q-step",
            &format!(r#"data-step="{}""#, index + 1),
        );
        let _ = writeln!(
            svg,
            r#"<rect x="{PADDING:.1}" y="{y:.1}" width="{content_width:.1}" height="{step_height:.1}" fill="{}" stroke="{LINE}" stroke-width="0.8"/>"#,
            if index % 2 == 0 { WHITE } else { SOFT }
        );
        centered(
            &mut svg,
            PADDING + 24.0,
            y + step_height / 2.0,
            12.0,
            700,
            &(index + 1).to_string(),
        );
        text_lines(&mut svg, PADDING + 50.0, y + 22.0, 15.0, 11.0, 400, lines);
        group_end(&mut svg);
        y += step_height;
    }
    if let Some(refusal) = refusal {
        let lines = wrap_text(&refusal, content_width - 20.0, 10.5);
        let box_height = lines.len() as f32 * 14.0 + 28.0;
        group_start(&mut svg, "q-refusal", "refusal", "");
        let _ = writeln!(
            svg,
            r#"<rect x="{PADDING:.1}" y="{y:.1}" width="{content_width:.1}" height="{box_height:.1}" fill="{SOFT}" stroke="{LINE}" stroke-width="1.2"/>"#
        );
        text_lines(&mut svg, PADDING + 10.0, y + 21.0, 14.0, 10.5, 500, &lines);
        group_end(&mut svg);
        y += box_height;
    }
    group_start(&mut svg, "q-terminal-result", "q-terminal-result", "");
    let _ = writeln!(
        svg,
        r#"<rect x="{PADDING:.1}" y="{:.1}" width="{content_width:.1}" height="56" fill="{WHITE}" stroke="{LINE}" stroke-width="1.5"/>"#,
        y + 12.0
    );
    text(&mut svg, PADDING + 10.0, y + 34.0, 10.0, 700, status);
    text(&mut svg, PADDING + 10.0, y + 53.0, 11.0, 400, &terminal);
    group_end(&mut svg);
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    Ok(svg)
}

#[derive(Debug, Clone)]
struct PlotSeries {
    id: String,
    label: String,
    values: Vec<(String, f64)>,
}

#[derive(Debug, Clone)]
struct PlotData {
    title: String,
    axis: String,
    note: String,
    provenance: String,
    series: Vec<PlotSeries>,
}

fn plot_data(document: &ConvalgenDocument) -> Result<PlotData, String> {
    let provenance = if document.source.source_locator.trim().is_empty() {
        "current project record".to_owned()
    } else {
        document.source.source_locator.clone()
    };
    match document.plot {
        PlotKind::ConstraintWeights => Ok(PlotData {
            title: "Constraint weights".to_owned(),
            axis: "weight; signed zero baseline shown".to_owned(),
            note: "Exact stored values are converted only for plotting; visible labels retain their canonical spellings.".to_owned(),
            provenance,
            series: vec![PlotSeries {
                id: "weights".to_owned(),
                label: "declared weight".to_owned(),
                values: document
                    .source
                    .constraints
                    .iter()
                    .filter_map(|constraint| {
                        constraint.weight.as_ref().and_then(|weight| {
                            weight
                                .to_f64_center()
                                .ok()
                                .map(|value| (constraint.name.clone(), value))
                        })
                    })
                    .collect(),
            }],
        }),
        PlotKind::SerialPath => {
            let result = PhonologicalEngine::new()
                .serial(
                    &document.source,
                    &document.serial,
                    document.evaluator,
                    document.temperature.to_f64_center().map_err(|error| error.to_string())?,
                )
                .map_err(|problem| problem.to_string())?;
            Ok(PlotData {
                title: "Serial stage path".to_owned(),
                axis: "stage number".to_owned(),
                note: format!("Stopping witness: {}", result.stopped),
                provenance,
                series: vec![PlotSeries {
                    id: "serial-stages".to_owned(),
                    label: "selected stage form".to_owned(),
                    values: result
                        .path
                        .iter()
                        .enumerate()
                        .map(|(index, form)| (format!("stage {index}: {form}"), index as f64))
                        .collect(),
                }],
            })
        }
        PlotKind::RankingShares => {
            let audit = PhonologicalEngine::new()
                .q_clone_audit(
                    &document.source,
                    document.clone_constraint,
                    &document.a_priori_rankings,
                    document.evaluator,
                    document.temperature.to_f64_center().map_err(|error| error.to_string())?,
                )
                .map_err(|problem| problem.to_string())?;
            let before = audit
                .shifts
                .iter()
                .map(|shift| {
                    (
                        shift.answer.join("; "),
                        shift.before.to_f64(),
                    )
                })
                .collect();
            let after = audit
                .shifts
                .iter()
                .map(|shift| {
                    (
                        shift.answer.join("; "),
                        shift.after.to_f64(),
                    )
                })
                .collect();
            Ok(PlotData {
                title: "Ranking-space shares".to_owned(),
                axis: "share of compatible strict rankings".to_owned(),
                note: format!(
                    "Exact finite denominators: source {}; transformed {}. Bars are decimal views of integer ratios.",
                    audit.before.total_rankings, audit.after.total_rankings
                ),
                provenance,
                series: vec![
                    PlotSeries {
                        id: "before".to_owned(),
                        label: "source".to_owned(),
                        values: before,
                    },
                    PlotSeries {
                        id: "after".to_owned(),
                        label: "after clone".to_owned(),
                        values: after,
                    },
                ],
            })
        }
        PlotKind::CandidateScores | PlotKind::CandidateProbabilities => {
            let evaluation = PhonologicalEngine::new()
                .evaluate_in_project(document, &document.source)
                .map_err(|problem| problem.to_string())?;
            let (title, axis, note, values) = if document.plot == PlotKind::CandidateProbabilities {
                let sum: f64 = evaluation
                    .rows
                    .iter()
                    .map(|row| row.probability.unwrap_or(0.0))
                    .sum();
                (
                    "Candidate probabilities".to_owned(),
                    "conditional probability".to_owned(),
                    format!(
                        "Complete displayed support n = {}; ΣP ≈ {}; independent normalizer; probabilities rounded to 6 decimals.",
                        document.source.candidates.len(),
                        compact_decimal(sum, 9)
                    ),
                    evaluation
                        .rows
                        .iter()
                        .map(|row| {
                            (
                                document.source.candidates[row.candidate].name.clone(),
                                row.probability.unwrap_or(0.0),
                            )
                        })
                        .collect(),
                )
            } else if document.evaluator == EvaluatorKind::Ot {
                (
                    "Candidate rank tiers".to_owned(),
                    "lexicographic rank tier; lower is better".to_owned(),
                    "Finite strict-OT ordering; the plotted tier index is discrete.".to_owned(),
                    evaluation
                        .rows
                        .iter()
                        .map(|row| {
                            let tier = evaluation
                                .ordered_strata
                                .iter()
                                .position(|tier| tier.contains(&row.candidate))
                                .map(|index| index + 1)
                                .unwrap_or(0);
                            (
                                document.source.candidates[row.candidate].name.clone(),
                                tier as f64,
                            )
                        })
                        .collect(),
                )
            } else {
                (
                    "Candidate costs".to_owned(),
                    "weighted violation cost; lower is better".to_owned(),
                    "Exact stored weighted costs remain exact; the plot uses their floating centers only to place marks on the axis.".to_owned(),
                    evaluation
                        .rows
                        .iter()
                        .map(|row| {
                            (
                                document.source.candidates[row.candidate].name.clone(),
                                row.harmony,
                            )
                        })
                        .collect(),
                )
            };
            Ok(PlotData {
                title,
                axis,
                note,
                provenance,
                series: vec![PlotSeries {
                    id: "candidate-values".to_owned(),
                    label: document.plot.label_for(document.evaluator).to_owned(),
                    values,
                }],
            })
        }
    }
}

pub fn plot_svg(document: &ConvalgenDocument) -> Result<String, String> {
    let data = plot_data(document)?;
    if data.series.is_empty() || data.series.iter().all(|series| series.values.is_empty()) {
        return Err("plot export refused: the selected plot has no finite values".to_owned());
    }
    let mut labels = Vec::new();
    for series in &data.series {
        for (label, _) in &series.values {
            if !labels.contains(label) {
                labels.push(label.clone());
            }
        }
    }
    let values = data
        .series
        .iter()
        .flat_map(|series| series.values.iter().map(|(_, value)| *value))
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err("plot export refused: every selected value is nonfinite".to_owned());
    }
    let minimum = values.iter().copied().fold(0.0_f64, f64::min);
    let maximum = values.iter().copied().fold(0.0_f64, f64::max);
    let span = (maximum - minimum).max(f64::MIN_POSITIVE);
    let label_width = labels
        .iter()
        .map(|label| measure_text(label, 10.5, 500))
        .fold(130.0_f32, f32::max)
        .clamp(130.0, 300.0);
    let chart_width = 560.0;
    let content_width = label_width + chart_width + 82.0;
    let title_lines = wrap_text(&data.title, content_width, 19.0);
    let subtitle_lines = if document.presentation.show_title && data.title != document.title {
        wrap_text(&document.title, content_width, 11.0)
    } else {
        Vec::new()
    };
    let note_lines = wrap_text(&data.note, content_width, 10.0);
    let provenance_lines = wrap_text(
        &format!("data provenance: {}", data.provenance),
        content_width,
        9.5,
    );
    let series_count = data.series.len().max(1);
    let category_height = 18.0 + series_count as f32 * 17.0;
    let header_height = 64.0
        + title_lines.len() as f32 * 22.0
        + subtitle_lines.len() as f32 * 14.0
        + note_lines.len() as f32 * 13.0
        + provenance_lines.len() as f32 * 12.0
        + data.series.len() as f32 * 18.0;
    let chart_height = labels.len() as f32 * category_height + 50.0;
    let width = content_width + PADDING * 2.0;
    let height = header_height + chart_height + PADDING * 2.0;
    let mut svg = svg_start(
        width,
        height,
        &data.title,
        &format!(
            "Monochrome plot with labelled axis {}, explicit zero baseline, provenance, and {} distinguishable series.",
            data.axis,
            data.series.len()
        ),
    );
    group_start(
        &mut svg,
        "plot",
        "plot",
        &format!(r#"data-kind="{}""#, document.plot.label()),
    );
    let mut y = PADDING + 18.0;
    text_lines(&mut svg, PADDING, y, 22.0, 19.0, 700, &title_lines);
    y += title_lines.len() as f32 * 22.0 + 4.0;
    if !subtitle_lines.is_empty() {
        text_lines(&mut svg, PADDING, y, 14.0, 11.0, 500, &subtitle_lines);
        y += subtitle_lines.len() as f32 * 14.0 + 3.0;
    }
    text_lines(&mut svg, PADDING, y, 13.0, 10.0, 400, &note_lines);
    y += note_lines.len() as f32 * 13.0 + 3.0;
    text_lines(&mut svg, PADDING, y, 12.0, 9.5, 400, &provenance_lines);
    y += provenance_lines.len() as f32 * 12.0 + 8.0;
    group_start(&mut svg, "plot-legend", "plot-legend", "");
    for (index, series) in data.series.iter().enumerate() {
        let fill = match index % 3 {
            0 => LINE,
            1 => "url(#hatch-diagonal)",
            _ => "url(#hatch-cross)",
        };
        rect(&mut svg, PADDING + 2.0, y - 10.0, 18.0, 10.0, fill);
        text(&mut svg, PADDING + 27.0, y, 10.0, 500, &series.label);
        y += 18.0;
    }
    group_end(&mut svg);

    let chart_left = PADDING + label_width + 22.0;
    let chart_right = chart_left + chart_width;
    let chart_top = y + 20.0;
    let chart_bottom = chart_top + labels.len() as f32 * category_height;
    let x_for = |value: f64| chart_left + ((value - minimum) / span) as f32 * chart_width;
    for tick in 0..=5 {
        let value = minimum + span * tick as f64 / 5.0;
        let x = x_for(value);
        let _ = writeln!(
            svg,
            r#"<line x1="{x:.1}" y1="{chart_top:.1}" x2="{x:.1}" y2="{chart_bottom:.1}" stroke="{MID}" stroke-width="0.6" stroke-dasharray="2 3"/>"#
        );
        centered(
            &mut svg,
            x,
            chart_bottom + 18.0,
            9.5,
            400,
            &compact_decimal(value, 4),
        );
    }
    let zero_x = x_for(0.0_f64.clamp(minimum, maximum));
    line(&mut svg, zero_x, chart_top, zero_x, chart_bottom, 1.5);
    line(
        &mut svg,
        chart_left,
        chart_bottom,
        chart_right,
        chart_bottom,
        1.0,
    );

    for (category_index, label) in labels.iter().enumerate() {
        let row_top = chart_top + category_index as f32 * category_height;
        let label_lines = wrap_text(label, label_width, 10.5);
        centered_lines(
            &mut svg,
            PADDING + label_width / 2.0,
            row_top + category_height / 2.0,
            12.0,
            10.5,
            500,
            &label_lines,
        );
        for (series_index, series) in data.series.iter().enumerate() {
            let value = series
                .values
                .iter()
                .find(|(candidate, _)| candidate == label)
                .map(|(_, value)| *value)
                .unwrap_or(0.0);
            let value_x = x_for(value);
            let left = value_x.min(zero_x);
            let bar_width = (value_x - zero_x).abs().max(1.0);
            let bar_y = row_top + 8.0 + series_index as f32 * 17.0;
            let fill = match series_index % 3 {
                0 => LINE,
                1 => "url(#hatch-diagonal)",
                _ => "url(#hatch-cross)",
            };
            let group_id = format!(
                "plot-series-{}-item-{category_index}",
                xml_id("series", &series.id)
            );
            group_start(
                &mut svg,
                &group_id,
                "plot-series-item",
                &format!(r#"data-value="{}""#, compact_decimal(value, 12)),
            );
            rect(&mut svg, left, bar_y, bar_width, 11.0, fill);
            let label_x = if value >= 0.0 {
                value_x + 5.0
            } else {
                value_x - 5.0
            };
            if value >= 0.0 {
                text(
                    &mut svg,
                    label_x,
                    bar_y + 10.0,
                    9.5,
                    500,
                    &compact_decimal(value, 6),
                );
            } else {
                right_text(
                    &mut svg,
                    label_x,
                    bar_y + 10.0,
                    9.5,
                    500,
                    &compact_decimal(value, 6),
                );
            }
            group_end(&mut svg);
        }
    }
    centered(
        &mut svg,
        (chart_left + chart_right) / 2.0,
        chart_bottom + 38.0,
        10.5,
        600,
        &data.axis,
    );
    group_end(&mut svg);
    svg.push_str("</svg>\n");
    Ok(svg)
}

fn load_resvg_fonts(options: &mut resvg::usvg::Options<'_>) {
    options
        .fontdb_mut()
        .load_font_data(ttf_noto_sans::REGULAR.to_vec());
    options
        .fontdb_mut()
        .load_font_data(ttf_noto_sans::BOLD.to_vec());
    options
        .fontdb_mut()
        .load_font_data(rwml_fonts::noto_sans_arabic_subset().to_vec());
    options
        .fontdb_mut()
        .load_font_data(epaint_default_fonts::EMOJI_ICON.to_vec());
}

fn load_svg2pdf_fonts(options: &mut svg2pdf::usvg::Options<'_>) {
    options
        .fontdb_mut()
        .load_font_data(ttf_noto_sans::REGULAR.to_vec());
    options
        .fontdb_mut()
        .load_font_data(ttf_noto_sans::BOLD.to_vec());
    options
        .fontdb_mut()
        .load_font_data(rwml_fonts::noto_sans_arabic_subset().to_vec());
    options
        .fontdb_mut()
        .load_font_data(epaint_default_fonts::EMOJI_ICON.to_vec());
}

// Semantic pagination metadata embedded in a single native tableau export.
#[derive(Debug, Clone)]
struct SemanticPublicationLayout {
    table_left: f32,
    header_top: f32,
    body_top: f32,
    pinned_right: f32,
    column_breaks: Vec<f32>,
    row_breaks: Vec<f32>,
    constraint_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SemanticSlice {
    start_index: usize,
    end_index: usize,
    start: f32,
    end: f32,
}

fn svg_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!(r#"{name}=""#);
    let start = tag.find(&prefix)? + prefix.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

fn parse_publication_number(tag: &str, name: &str) -> Result<f32, String> {
    let source = svg_attribute(tag, name)
        .ok_or_else(|| format!("publication tiling refused: missing {name}"))?;
    let value = source
        .parse::<f32>()
        .map_err(|_| format!("publication tiling refused: invalid {name} `{source}`"))?;
    if !value.is_finite() {
        return Err(format!("publication tiling refused: {name} must be finite"));
    }
    Ok(value)
}

fn parse_publication_breaks(tag: &str, name: &str) -> Result<Vec<f32>, String> {
    let source = svg_attribute(tag, name)
        .ok_or_else(|| format!("publication tiling refused: missing {name}"))?;
    let values = source
        .split_whitespace()
        .map(|item| {
            item.parse::<f32>().map_err(|_| {
                format!("publication tiling refused: invalid {name} coordinate `{item}`")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() < 2
        || values.iter().any(|value| !value.is_finite())
        || values
            .windows(2)
            .any(|pair| pair[1] <= pair[0] || pair[1] - pair[0] < 0.1)
    {
        return Err(format!(
            "publication tiling refused: {name} must contain at least one positive, strictly increasing cell interval"
        ));
    }
    Ok(values)
}

fn semantic_publication_layout(
    svg: &str,
    source_width: f32,
    source_height: f32,
) -> Result<Option<SemanticPublicationLayout>, String> {
    let marker = r#"data-publication-layout="tableau""#;
    let mut occurrences = svg.match_indices(marker);
    let Some((marker_start, _)) = occurrences.next() else {
        return Ok(None);
    };
    // Composite Second-Order and serial exports can contain multiple
    // tableaux. One pinned row/column register cannot describe several
    // independent grids, so refuse fixed-page tiling instead of silently
    // bisecting one of them. Their native content-cropped export is unaffected.
    if occurrences.next().is_some() {
        return Err(
            "publication tiling refused: the export contains multiple independent tableaux; export each tableau separately or use the complete native vector/PDF so no cell is bisected"
                .to_owned(),
        );
    }
    let tag_start = svg[..marker_start].rfind('<').ok_or_else(|| {
        "publication tiling refused: tableau publication metadata has no opening tag".to_owned()
    })?;
    let tag_end = svg[marker_start..]
        .find('>')
        .map(|offset| marker_start + offset)
        .ok_or_else(|| {
            "publication tiling refused: tableau publication metadata is incomplete".to_owned()
        })?;
    let tag = &svg[tag_start..=tag_end];
    let table_left = parse_publication_number(tag, "data-publication-table-left")?;
    let header_top = parse_publication_number(tag, "data-publication-header-top")?;
    let body_top = parse_publication_number(tag, "data-publication-body-top")?;
    let pinned_right = parse_publication_number(tag, "data-publication-pinned-right")?;
    let column_breaks = parse_publication_breaks(tag, "data-publication-column-breaks")?;
    let row_breaks = parse_publication_breaks(tag, "data-publication-row-breaks")?;
    let constraint_count_source = svg_attribute(tag, "data-publication-constraint-count")
        .ok_or_else(|| {
            "publication tiling refused: missing data-publication-constraint-count".to_owned()
        })?;
    let constraint_count = constraint_count_source.parse::<usize>().map_err(|_| {
        format!(
            "publication tiling refused: invalid data-publication-constraint-count `{constraint_count_source}`"
        )
    })?;
    let approximately_equal = |left: f32, right: f32| (left - right).abs() <= 0.11;
    if table_left < 0.0
        || header_top < 0.0
        || body_top <= header_top
        || pinned_right <= table_left
        || !approximately_equal(column_breaks[0], pinned_right)
        || !approximately_equal(row_breaks[0], body_top)
        || column_breaks.last().copied().unwrap_or_default() > source_width + 0.11
        || row_breaks.last().copied().unwrap_or_default() > source_height + 0.11
        || constraint_count > column_breaks.len() - 1
    {
        return Err(
            "publication tiling refused: tableau publication coordinates are inconsistent with the native canvas"
                .to_owned(),
        );
    }
    Ok(Some(SemanticPublicationLayout {
        table_left,
        header_top,
        body_top,
        pinned_right,
        column_breaks,
        row_breaks,
        constraint_count,
    }))
}

fn semantic_slices(
    breaks: &[f32],
    capacity: f32,
    unit: &str,
) -> Result<Vec<SemanticSlice>, String> {
    let mut slices = Vec::new();
    let mut start_index = 0;
    while start_index + 1 < breaks.len() {
        let mut end_index = start_index + 1;
        if breaks[end_index] - breaks[start_index] > capacity + 0.11 {
            return Err(format!(
                "publication tiling refused: one {unit} is {:.1} units wide/high but only {capacity:.1} units are available; refusing to bisect it",
                breaks[end_index] - breaks[start_index]
            ));
        }
        while end_index + 1 < breaks.len()
            && breaks[end_index + 1] - breaks[start_index] <= capacity + 0.11
        {
            end_index += 1;
        }
        slices.push(SemanticSlice {
            start_index,
            end_index,
            start: breaks[start_index],
            end: breaks[end_index],
        });
        start_index = end_index;
    }
    Ok(slices)
}

fn publication_fragment(
    page: &mut String,
    class: &str,
    target: (f32, f32),
    source: (f32, f32),
    size: (f32, f32),
) {
    let (target_x, target_y) = target;
    let (source_x, source_y) = source;
    let (width, height) = size;
    let _ = writeln!(
        page,
        r##"<svg class="{class}" x="{target_x:.1}" y="{target_y:.1}" width="{width:.1}" height="{height:.1}" viewBox="{source_x:.1} {source_y:.1} {width:.1} {height:.1}" overflow="hidden"><use href="#publication-source"/></svg>"##
    );
}

fn semantic_publication_tiles(
    inner: &str,
    layout: &SemanticPublicationLayout,
    page_width: f32,
    page_height: f32,
    margin: f32,
) -> Result<Vec<PublicationTile>, String> {
    const PAGE_HEADER_HEIGHT: f32 = 24.0;
    let content_width = page_width - margin * 2.0;
    let content_height = page_height - margin * 2.0;
    let pinned_width = layout.pinned_right - layout.table_left;
    let header_height = layout.body_top - layout.header_top;
    let analysis_capacity = content_width - pinned_width;
    let row_capacity = content_height - PAGE_HEADER_HEIGHT - header_height;
    if analysis_capacity <= 0.0 || row_capacity <= 0.0 {
        return Err(
            "publication tiling refused: page content area cannot hold the repeated candidate and input/header registers"
                .to_owned(),
        );
    }
    let column_slices = semantic_slices(
        &layout.column_breaks,
        analysis_capacity,
        "constraint/metric column",
    )?;
    let row_slices = semantic_slices(&layout.row_breaks, row_capacity, "candidate row")?;
    let columns = column_slices.len();
    let rows = row_slices.len();
    let page_count = rows.saturating_mul(columns);
    if page_count > 256 {
        return Err(format!(
            "publication tiling refused: {rows} × {columns} = {page_count} pages exceeds the 256-page safety limit"
        ));
    }

    let mut result = Vec::with_capacity(page_count);
    for (row, row_slice) in row_slices.iter().enumerate() {
        for (column, column_slice) in column_slices.iter().enumerate() {
            let page_number = row * columns + column + 1;
            let row_first = row_slice.start_index + 1;
            let row_last = row_slice.end_index;
            let column_first = column_slice.start_index + 1;
            let column_last = column_slice.end_index;
            let constraint_last = column_last.min(layout.constraint_count);
            let column_summary = if column_first <= layout.constraint_count {
                if column_last <= layout.constraint_count {
                    format!("constraints {column_first}–{column_last}")
                } else {
                    format!("constraints {column_first}–{constraint_last} and computed metrics")
                }
            } else {
                "computed metrics".to_owned()
            };
            let analysis_width = column_slice.end - column_slice.start;
            let body_height = row_slice.end - row_slice.start;
            let header_y = margin + PAGE_HEADER_HEIGHT;
            let body_y = header_y + header_height;
            let analysis_x = margin + pinned_width;
            let mut page = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_width:.1}" height="{page_height:.1}" viewBox="0 0 {page_width:.1} {page_height:.1}" role="img" aria-labelledby="publication-title publication-description" data-mode="publication-tiles" data-break-policy="semantic-cells" data-page="{page_number}" data-pages="{page_count}" data-row-start="{row_first}" data-row-end="{row_last}" data-column-start="{column_first}" data-column-end="{column_last}">
<title id="publication-title">Publication tile {page_number} of {page_count}</title>
<desc id="publication-description">Candidate rows {row_first} through {row_last}; {column_summary}; repeated input, constraint-header, and candidate registers.</desc>
<rect x="0" y="0" width="{page_width:.1}" height="{page_height:.1}" fill="{WHITE}"/>
<defs><g id="publication-source">
"#
            );
            page.push_str(inner);
            page.push_str("</g></defs>\n");
            text(
                &mut page,
                margin,
                margin + 13.0,
                10.0,
                600,
                &format!("Candidates {row_first}–{row_last} · {column_summary}"),
            );
            publication_fragment(
                &mut page,
                "publication-input-header",
                (margin, header_y),
                (layout.table_left, layout.header_top),
                (pinned_width, header_height),
            );
            publication_fragment(
                &mut page,
                "publication-constraint-headers",
                (analysis_x, header_y),
                (column_slice.start, layout.header_top),
                (analysis_width, header_height),
            );
            publication_fragment(
                &mut page,
                "publication-candidate-register",
                (margin, body_y),
                (layout.table_left, row_slice.start),
                (pinned_width, body_height),
            );
            publication_fragment(
                &mut page,
                "publication-analysis-cells",
                (analysis_x, body_y),
                (column_slice.start, row_slice.start),
                (analysis_width, body_height),
            );
            right_text(
                &mut page,
                page_width - margin,
                page_height - 10.0,
                8.5,
                400,
                &format!(
                    "row {}/{} · column {}/{} · page {page_number}/{page_count}",
                    row + 1,
                    rows,
                    column + 1,
                    columns
                ),
            );
            page.push_str("</svg>\n");
            result.push(PublicationTile {
                row,
                column,
                rows,
                columns,
                svg: page,
            });
        }
    }
    Ok(result)
}

fn geometric_publication_tiles(
    inner: &str,
    source_width: f32,
    source_height: f32,
    page_width: f32,
    page_height: f32,
    margin: f32,
) -> Result<Vec<PublicationTile>, String> {
    let tile_width = page_width - margin * 2.0;
    let tile_height = page_height - margin * 2.0;
    let columns = (source_width / tile_width).ceil().max(1.0) as usize;
    let rows = (source_height / tile_height).ceil().max(1.0) as usize;
    let page_count = rows.saturating_mul(columns);
    if page_count > 256 {
        return Err(format!(
            "publication tiling refused: {rows} × {columns} = {page_count} pages exceeds the 256-page safety limit"
        ));
    }
    let mut result = Vec::with_capacity(page_count);
    for row in 0..rows {
        for column in 0..columns {
            let source_x = column as f32 * tile_width;
            let source_y = row as f32 * tile_height;
            let visible_width = (source_width - source_x).min(tile_width);
            let visible_height = (source_height - source_y).min(tile_height);
            let page_number = row * columns + column + 1;
            let mut page = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_width:.1}" height="{page_height:.1}" viewBox="0 0 {page_width:.1} {page_height:.1}" role="img" aria-labelledby="publication-title publication-description" data-mode="publication-tiles" data-break-policy="geometric" data-page="{page_number}" data-pages="{page_count}">
<title id="publication-title">Publication tile {page_number} of {page_count}</title>
<desc id="publication-description">Row {} of {rows}, column {} of {columns}; explicit paginated view of a complete PhonoScript GUI vector export.</desc>
<rect x="0" y="0" width="{page_width:.1}" height="{page_height:.1}" fill="{WHITE}"/>
<svg x="{margin:.1}" y="{margin:.1}" width="{visible_width:.1}" height="{visible_height:.1}" viewBox="{source_x:.1} {source_y:.1} {visible_width:.1} {visible_height:.1}" overflow="hidden">
"#,
                row + 1,
                column + 1,
            );
            page.push_str(inner);
            page.push_str("</svg>\n");
            right_text(
                &mut page,
                page_width - margin,
                page_height - 10.0,
                8.5,
                400,
                &format!(
                    "row {}/{} · column {}/{} · page {page_number}/{page_count}",
                    row + 1,
                    rows,
                    column + 1,
                    columns
                ),
            );
            page.push_str("</svg>\n");
            result.push(PublicationTile {
                row,
                column,
                rows,
                columns,
                svg: page,
            });
        }
    }
    Ok(result)
}

/// Split a large native SVG into an explicit fixed-page publication view.
///
/// The default exporter remains crop-to-content. This opt-in view preserves
/// the complete vector tree so text and semantic groups remain editable and
/// searchable. A single tableau carrying semantic publication metadata is
/// divided only at candidate-row and analysis-column boundaries; its input and
/// constraint header band and its candidate column are repeated on every page.
/// Non-tableau SVG families retain geometric clipping; composite exports with
/// several independent tableau grids are refused rather than cut unsafely.
/// Page dimensions and margins use the same SVG user units as the source
/// artifact.
pub fn publication_tiles(
    svg: &str,
    page_width: f32,
    page_height: f32,
    margin: f32,
) -> Result<Vec<PublicationTile>, String> {
    if !page_width.is_finite()
        || !page_height.is_finite()
        || !margin.is_finite()
        || page_width <= 0.0
        || page_height <= 0.0
        || margin < 0.0
        || margin * 2.0 >= page_width
        || margin * 2.0 >= page_height
    {
        return Err(
            "publication tiling refused: page dimensions and margin must define a positive finite content area"
                .to_owned(),
        );
    }
    let mut options = resvg::usvg::Options::default();
    load_resvg_fonts(&mut options);
    let tree = resvg::usvg::Tree::from_str(svg, &options)
        .map_err(|error| format!("publication tiling refused: invalid native SVG: {error}"))?;
    let source_width = tree.size().width();
    let source_height = tree.size().height();
    let opening_end = svg.find('>').ok_or_else(|| {
        "publication tiling refused: SVG opening element is incomplete".to_owned()
    })?;
    let closing_start = svg
        .rfind("</svg>")
        .ok_or_else(|| "publication tiling refused: SVG closing element is absent".to_owned())?;
    if closing_start <= opening_end {
        return Err("publication tiling refused: SVG element ordering is invalid".to_owned());
    }
    let inner = &svg[opening_end + 1..closing_start];
    if let Some(layout) = semantic_publication_layout(svg, source_width, source_height)? {
        semantic_publication_tiles(inner, &layout, page_width, page_height, margin)
    } else {
        geometric_publication_tiles(
            inner,
            source_width,
            source_height,
            page_width,
            page_height,
            margin,
        )
    }
}

pub fn write_publication_pdf_tiles(
    svg: &str,
    stem: &Path,
    page_width: f32,
    page_height: f32,
    margin: f32,
) -> Result<Vec<PathBuf>, String> {
    let tiles = publication_tiles(svg, page_width, page_height, margin)?;
    let mut paths = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let path = stem.with_file_name(format!(
            "{}-page-r{:02}-c{:02}",
            stem.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("publication"),
            tile.row + 1,
            tile.column + 1
        ));
        paths.push(write(&tile.svg, &path, ExportFormat::Pdf)?);
    }
    Ok(paths)
}

pub fn write(svg: &str, path: &Path, format: ExportFormat) -> Result<PathBuf, String> {
    write_with_scale(svg, path, format, 1.0)
}

pub fn write_with_scale(
    svg: &str,
    path: &Path,
    format: ExportFormat,
    png_scale: f32,
) -> Result<PathBuf, String> {
    let destination = path.with_extension(format.extension());
    match format {
        ExportFormat::Svg => fs::write(&destination, svg)
            .map_err(|error| format!("could not write {}: {error}", destination.display()))?,
        ExportFormat::Png => {
            if !png_scale.is_finite() || !(0.5..=4.0).contains(&png_scale) {
                return Err(format!(
                    "PNG export refused: scale {png_scale:?} is outside the declared 0.5× through 4× range"
                ));
            }
            let mut options = resvg::usvg::Options::default();
            load_resvg_fonts(&mut options);
            let tree = resvg::usvg::Tree::from_str(svg, &options)
                .map_err(|error| format!("could not parse generated SVG: {error}"))?;
            let size = tree.size().to_int_size();
            let width = (size.width() as f64 * f64::from(png_scale)).round();
            let height = (size.height() as f64 * f64::from(png_scale)).round();
            if width < 1.0
                || height < 1.0
                || width > f64::from(MAX_PNG_SIDE)
                || height > f64::from(MAX_PNG_SIDE)
                || width * height > MAX_PNG_PIXELS as f64
            {
                return Err(format!(
                    "PNG export refused: requested {width:.0} × {height:.0} pixels exceeds the {MAX_PNG_PIXELS}-pixel or {MAX_PNG_SIDE}-per-side safety limit; export SVG/PDF or choose a smaller explicit scale"
                ));
            }
            let width = width as u32;
            let height = height as u32;
            let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
                "PNG export refused: renderer could not allocate the requested image".to_owned()
            })?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(png_scale, png_scale),
                &mut pixmap.as_mut(),
            );
            let file = fs::File::create(&destination)
                .map_err(|error| format!("could not write {}: {error}", destination.display()))?;
            let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_pixel_dims(Some(png::PixelDimensions {
                xppu: (3_779.527_6_f32 * png_scale).round() as u32,
                yppu: (3_779.527_6_f32 * png_scale).round() as u32,
                unit: png::Unit::Meter,
            }));
            encoder
                .add_text_chunk(
                    "PhonoScript GUI export scale".to_owned(),
                    format!("{png_scale}x at 96 CSS pixels per inch"),
                )
                .map_err(|error| format!("could not describe PNG export: {error}"))?;
            let mut writer = encoder
                .write_header()
                .map_err(|error| format!("could not initialize PNG export: {error}"))?;
            writer
                .write_image_data(pixmap.data())
                .map_err(|error| format!("could not write {}: {error}", destination.display()))?;
            writer
                .finish()
                .map_err(|error| format!("could not finish {}: {error}", destination.display()))?;
        }
        ExportFormat::Pdf => {
            let mut options = svg2pdf::usvg::Options::default();
            load_svg2pdf_fonts(&mut options);
            let tree = svg2pdf::usvg::Tree::from_str(svg, &options)
                .map_err(|error| format!("could not parse generated SVG: {error}"))?;
            let pdf = svg2pdf::to_pdf(
                &tree,
                svg2pdf::ConversionOptions::default(),
                svg2pdf::PageOptions::default(),
            )
            .map_err(|error| format!("could not convert generated SVG: {error:?}"))?;
            fs::write(&destination, pdf)
                .map_err(|error| format!("could not write {}: {error}", destination.display()))?;
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PlotKind, SecondOrderLayout};
    use crate::reference_cases;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_stem(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let directory = option_env!("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tmp"));
        fs::create_dir_all(&directory).expect("workspace target temp directory");
        directory.join(format!(
            "convalgen-export-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn strict_ot_has_semantic_structure_and_no_phantom_trailing_cell() {
        let document = reference_cases::prince_smolensky_ot();
        let layout = table_layout(&document.source, &document);
        let svg = tableau_svg(&document, false).expect("strict OT export");
        assert!(svg.contains("class=\"candidate-row\""));
        assert!(svg.contains("class=\"constraint-column\""));
        assert!(svg.contains("data-boundary=\""));
        assert!(svg.contains("*!"));
        assert!(svg.contains("☞"));
        assert!(!svg.contains("unresolved co-optimum"));
        assert!(!svg.contains(">Output<"));
        assert!(svg.contains(&format!("data-width=\"{:.1}\"", layout.width)));
        assert_eq!(
            layout.width,
            layout.candidate_width + BOUNDARY_GAP + layout.constraint_widths.iter().sum::<f32>()
        );
    }

    #[test]
    fn hg_and_maxent_keep_their_distinct_mathematics() {
        let mut hg_document = reference_cases::pater_hg();
        hg_document.source.constraints[0].weight =
            Some(NumericScalar::parse_exact("1000001/1000000").unwrap());
        let hg_layout = table_layout(&hg_document.source, &hg_document);
        for candidate in &hg_document.source.candidates {
            let value = exact_cost(&hg_document.source, candidate).unwrap();
            assert!(
                measure_text(&value, 11.5, 400) + 28.0 <= hg_layout.metric_widths[0] + 0.1,
                "exact cost {value} must fit its computed metric column"
            );
        }
        let hg = tableau_svg(&hg_document, false).expect("HG export");
        assert!(hg.contains("cost ↓"));
        assert!(hg.contains("1000001/1000000"));
        assert!(!hg.contains("*!"));

        let mut maxent = reference_cases::finite_maxent_smoke();
        maxent.source.candidates[0].base_mass = NumericScalar::parse_exact("3/2").unwrap();
        let maxent = tableau_svg(&maxent, false).expect("MaxEnt export");
        for label in ["E ↓", "ρ", "u", "P", "Z ≈", "rounded to 6 decimals"] {
            assert!(maxent.contains(label), "missing {label}");
        }
        assert!(maxent.contains("3/2"));
        assert!(!maxent.contains("*!"));
    }

    #[test]
    fn every_second_order_layout_preserves_contract_and_first_order_evidence() {
        assert_eq!(comparison_symbol(ComparisonStatus::NotEvaluated), None);
        for layout in SecondOrderLayout::ALL {
            let mut document = reference_cases::dissertation_second_order();
            document.second_order.layout = layout;
            let svg = tableau_svg(&document, true).expect("Second-Order export");
            assert!(svg.contains("id=\"second-order-contract\""));
            assert!(svg.contains("SOURCE ANSWER"));
            assert!(svg.contains("TRANSPORTED SOURCE ANSWER"));
            assert!(svg.contains("TARGET ANSWER"));
            assert!(svg.contains("FORMATION / ADMISSION"));
            assert!(svg.matches("class=\"candidate-row\"").count() >= 6);
            assert!(svg.contains(&format!("data-layout=\"{}\"", layout.label())));
            assert!(!svg.contains("NORMALIZER"));
        }
    }

    #[test]
    fn serial_q_and_all_plot_families_emit_typed_semantic_groups() {
        assert_eq!(
            serial_panel_count(&SerialResult {
                path: vec!["a".into(), "b".into(), "a".into()],
                operations: vec!["a→b".into(), "b→a".into()],
                stopped: "refused: cycle detected".into(),
                formed: false,
            }),
            2,
            "a repeated cycle endpoint is not a third evaluated stage"
        );
        assert_eq!(
            serial_panel_count(&SerialResult {
                path: vec!["a".into(), "b".into()],
                operations: vec!["a→b".into()],
                stopped: "faithful convergence".into(),
                formed: true,
            }),
            2,
            "faithful convergence retains its final identity panel"
        );
        let serial =
            serial_svg(&reference_cases::serial_syllabification_smoke()).expect("serial export");
        assert!(serial.contains("id=\"serial-stopping-witness\""));
        assert!(serial.contains("class=\"serial-stage\""));
        assert!(serial.contains("selected stage winner"));

        let mut single_candidate = reference_cases::prince_smolensky_ot();
        single_candidate.source.candidates.truncate(1);
        let single_candidate =
            tableau_svg(&single_candidate, false).expect("single-candidate tableau export");
        assert!(single_candidate.contains("1 candidate under"));
        assert!(!single_candidate.contains("1 candidates"));

        let ranking = reference_cases::finnish_ranking_space();
        let q = q_calculus_svg(&ranking).expect("Q export");
        assert!(q.contains("id=\"q-calculus-derivation\""));
        assert!(q.contains("id=\"q-terminal-result\""));
        assert!(q.contains("data-tableau-ref"));

        for kind in PlotKind::ALL {
            let mut document = match kind {
                PlotKind::SerialPath => reference_cases::serial_syllabification_smoke(),
                PlotKind::RankingShares => reference_cases::finnish_ranking_space(),
                PlotKind::CandidateProbabilities => reference_cases::finite_maxent_smoke(),
                PlotKind::ConstraintWeights | PlotKind::CandidateScores => {
                    reference_cases::pater_hg()
                }
            };
            document.plot = kind;
            let svg = plot_svg(&document).expect("plot export");
            assert!(svg.contains("id=\"plot\""));
            assert!(svg.contains("id=\"plot-legend\""));
            assert!(svg.contains("data provenance:"));
            assert!(svg.contains("plot-series-item"));
        }
    }

    #[test]
    fn grapheme_wrapping_keeps_stacked_combining_marks_with_their_base() {
        let value = "a\u{0301}\u{0325}\u{0330}a\u{0301}\u{0325}\u{0330}";
        let lines = break_token(value, measure_text("a", 12.0, 400) * 1.1, 12.0);
        assert_eq!(lines.concat(), value);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.starts_with('a')));
    }

    #[test]
    fn native_writes_are_real_cropped_formats_and_never_tex() {
        let document = reference_cases::prince_smolensky_ot();
        let svg = tableau_svg(&document, false).expect("SVG");
        let stem = temporary_stem("formats");
        let svg_path = write(&svg, &stem.with_extension("tex"), ExportFormat::Svg).unwrap();
        let png_path = write_with_scale(&svg, &stem, ExportFormat::Png, 2.0).unwrap();
        let pdf_path = write(&svg, &stem, ExportFormat::Pdf).unwrap();
        assert_eq!(
            svg_path.extension().and_then(|value| value.to_str()),
            Some("svg")
        );
        assert!(fs::read_to_string(&svg_path).unwrap().starts_with("<svg"));
        let png = fs::read(&png_path).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(
            png.windows("PhonoScript GUI export scale".len())
                .any(|window| window == b"PhonoScript GUI export scale")
        );
        assert!(
            png.windows("2x at 96 CSS pixels per inch".len())
                .any(|window| window == b"2x at 96 CSS pixels per inch")
        );
        assert_eq!(&fs::read(&pdf_path).unwrap()[..5], b"%PDF-");
        for path in [svg_path, png_path, pdf_path] {
            let _ = fs::remove_file(path);
        }
    }

    #[test]
    fn oversized_or_implicit_raster_requests_are_refused() {
        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="50000" height="50000" viewBox="0 0 50000 50000"><rect width="1" height="1"/></svg>"#;
        let stem = temporary_stem("raster-refusal");
        let error = write_with_scale(huge, &stem, ExportFormat::Png, 1.0).unwrap_err();
        assert!(error.contains("PNG export refused"));
        let error = write_with_scale(huge, &stem, ExportFormat::Png, 0.25).unwrap_err();
        assert!(error.contains("outside the declared"));
    }

    #[test]
    fn publication_mode_is_explicit_and_tiles_without_changing_default_crop() {
        let svg = tableau_svg(&reference_cases::pater_hg(), false).unwrap();
        let tiles = publication_tiles(&svg, 520.0, 220.0, 20.0).unwrap();
        let repeated = publication_tiles(&svg, 520.0, 220.0, 20.0).unwrap();
        assert!(tiles.len() > 1);
        assert_eq!(tiles.len(), repeated.len());
        assert!(
            tiles
                .iter()
                .zip(&repeated)
                .all(|(left, right)| left.svg == right.svg)
        );
        assert!(tiles.iter().all(|tile| {
            tile.svg.contains("data-mode=\"publication-tiles\"")
                && tile.svg.contains("data-break-policy=\"semantic-cells\"")
                && tile.svg.contains("width=\"520.0\"")
                && tile.svg.contains("height=\"220.0\"")
                && tile.svg.contains("overflow=\"hidden\"")
                && tile.svg.contains("class=\"publication-input-header\"")
                && tile
                    .svg
                    .contains("class=\"publication-constraint-headers\"")
                && tile
                    .svg
                    .contains("class=\"publication-candidate-register\"")
                && tile.svg.contains("class=\"publication-analysis-cells\"")
                && tile
                    .svg
                    .matches("<use href=\"#publication-source\"")
                    .count()
                    == 4
                && tile.svg.matches("id=\"publication-source\"").count() == 1
                && !tile.svg.contains("width=\"100%\"")
                && !tile.svg.contains("height=\"100%\"")
        }));
        let stem = temporary_stem("semantic-publication");
        let pdf = write(&tiles[0].svg, &stem, ExportFormat::Pdf).unwrap();
        assert_eq!(&fs::read(&pdf).unwrap()[..5], b"%PDF-");
        let _ = fs::remove_file(pdf);
        assert!(svg.contains("data-crop=\"content\""));
        assert!(svg.contains("id=\"background\" x=\"0\" y=\"0\""));
        assert!(svg.contains("data-publication-layout=\"tableau\""));
        assert!(!svg.contains("width=\"100%\""));
        assert!(!svg.contains("height=\"100%\""));
        assert!(!svg.contains("publication-tiles"));
    }

    #[test]
    fn semantic_page_planning_is_deterministic_and_uses_only_cell_boundaries() {
        let breaks = [0.0, 80.0, 170.0, 260.0, 350.0];
        let expected = vec![
            SemanticSlice {
                start_index: 0,
                end_index: 2,
                start: 0.0,
                end: 170.0,
            },
            SemanticSlice {
                start_index: 2,
                end_index: 4,
                start: 170.0,
                end: 350.0,
            },
        ];
        assert_eq!(semantic_slices(&breaks, 180.0, "cell").unwrap(), expected);
        assert_eq!(semantic_slices(&breaks, 180.0, "cell").unwrap(), expected);
        let error = semantic_slices(&breaks, 70.0, "cell").unwrap_err();
        assert!(error.contains("refusing to bisect it"));
    }

    #[test]
    fn publication_tiling_refuses_multiple_independent_tableau_grids() {
        let svg = tableau_svg(&reference_cases::dissertation_second_order(), true).unwrap();
        let error = publication_tiles(&svg, 1_191.0, 842.0, 36.0).unwrap_err();
        assert!(error.contains("multiple independent tableaux"));
        assert!(error.contains("no cell is bisected"));
        assert!(svg.contains("data-crop=\"content\""));
    }

    #[test]
    fn embedded_fonts_and_accessibility_metadata_are_self_contained() {
        let svg = tableau_svg(&reference_cases::prince_smolensky_ot(), false).unwrap();
        assert!(svg.contains("<title id=\"export-title\">"));
        assert!(svg.contains("<desc id=\"export-description\">"));
        assert!(svg.contains("font-family: 'Noto Sans'"));
        assert!(svg.contains("font-family: 'emoji'"));
        assert!(svg.contains("font-family: 'Noto Sans Arabic'"));
        assert!(svg.matches("data:font/ttf;base64,").count() == 4);
        assert!(!svg.contains("Times New Roman"));
    }

    #[test]
    fn bundled_faces_cover_the_declared_export_symbols() {
        let text = rustybuzz::Face::from_slice(ttf_noto_sans::REGULAR, 0).unwrap();
        for character in [
            'ḥ', 'ɬ', 'ʈ', 'ɳ', 'ɲ', 'ʁ', 'ɐ', 'ỹ', '→', '↓', '≠', 'Σ', 'ρ', '▲',
        ] {
            assert!(
                text.glyph_index(character).is_some(),
                "text face lacks U+{:04X}",
                character as u32
            );
        }
        let symbols = rustybuzz::Face::from_slice(epaint_default_fonts::EMOJI_ICON, 0).unwrap();
        assert!(symbols.glyph_index('☞').is_some());
        let arabic = rustybuzz::Face::from_slice(rwml_fonts::noto_sans_arabic_subset(), 0).unwrap();
        for character in "سلام".chars() {
            assert!(arabic.glyph_index(character).is_some());
        }
    }
}
