use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use phonoscript::{
    document,
    model::ConvalgenDocument,
    phonoscript_frontend::{Severity, Span},
    phonoscript_runtime::{self, Number, RunResult, RuntimeDiagnostic, SelectedTableau, Value},
};
use serde::Serialize;

const HELP: &str = "PhonoScript 3 — constraint-based phonological analysis

Usage:
  phonoscript [OPTIONS] SCRIPT.phont [BASE.ottab]
  phonoscript [OPTIONS] -
  phonoscript --emit PROJECT.ottab [--write SCRIPT.phont]

Options:
  --base PATH       Start from a self-contained .ottab project
  --module-root PATH  Confine local .phont imports beneath this directory
  --write PATH      Save the resulting project as .ottab after successful execution
  --emit PATH       Render an .ottab project as self-contained PhonoScript
  --check           Parse and statically analyse without executing
  --json            Emit one structured JSON result instead of the text report
  --quiet           Suppress the text report (errors still use stderr)
  -h, --help        Show this help
  -V, --version     Show the interpreter and language versions

Use '-' as the script path to read UTF-8 PhonoScript from standard input.";

#[derive(Serialize)]
struct JsonPosition {
    byte: usize,
    line: usize,
    column: usize,
}

#[derive(Serialize)]
struct JsonSpan {
    start: JsonPosition,
    end: JsonPosition,
}

#[derive(Serialize)]
struct JsonRelatedSpan<'a> {
    source: &'a str,
    span: JsonSpan,
    message: &'a str,
}

#[derive(Serialize)]
struct JsonCallFrame<'a> {
    function: &'a str,
    source: &'a str,
    span: JsonSpan,
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    source: &'a str,
    code: &'a str,
    severity: &'static str,
    message: &'a str,
    primary: JsonSpan,
    related: Vec<JsonRelatedSpan<'a>>,
    call_stack: Vec<JsonCallFrame<'a>>,
    // Retained as convenience aliases for existing automation consumers.
    line: usize,
    column: usize,
    help: Option<&'a str>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonValue {
    ExactNumber {
        numerator: String,
        denominator: String,
    },
    ApproximateNumber {
        value: f64,
    },
    Boolean {
        value: bool,
    },
    Text {
        value: String,
    },
    List {
        items: Vec<JsonValue>,
    },
    Record {
        fields: BTreeMap<String, JsonValue>,
    },
    Null,
}

#[derive(Serialize)]
struct JsonBoundary<'a> {
    coordinate: &'a str,
    exact_value: String,
    engine_value: f64,
    line: usize,
    column: usize,
}

#[derive(Serialize)]
struct JsonStatistics {
    modules_loaded: usize,
    steps: u64,
    statements: u64,
    expressions: u64,
    calls: u64,
    loop_iterations: u64,
    engine_calls: u64,
    exact_to_engine_conversions: u64,
    queued_file_effects: usize,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    status: &'static str,
    language: &'static str,
    language_version: u32,
    source: &'a str,
    selected_tableau: String,
    output: &'a [String],
    value: JsonValue,
    rendered_value: String,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    approximate_boundaries: Vec<JsonBoundary<'a>>,
    statistics: JsonStatistics,
    document: &'a ConvalgenDocument,
}

#[derive(Serialize)]
struct JsonCheckReport<'a> {
    status: &'static str,
    language: &'static str,
    language_version: u32,
    source: &'a str,
    diagnostics: Vec<JsonDiagnostic<'a>>,
}

#[derive(Debug, Default)]
struct Options {
    script: Option<PathBuf>,
    base: Option<PathBuf>,
    module_root: Option<PathBuf>,
    write: Option<PathBuf>,
    emit: Option<PathBuf>,
    check: bool,
    json: bool,
    quiet: bool,
}

fn main() {
    if let Err(error) = run() {
        if !error.is_empty() {
            eprintln!("PhonoScript: {error}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
    {
        println!("{HELP}");
        return Ok(());
    }
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-V" | "--version"))
    {
        println!(
            "phonoscript {} (language {})",
            env!("CARGO_PKG_VERSION"),
            phonoscript_runtime::LANGUAGE_VERSION
        );
        return Ok(());
    }
    let options = parse_arguments(&arguments)?;
    if let Some(project_path) = &options.emit {
        let document = document::load(project_path)?;
        let source = phonoscript_runtime::try_emit(&document)?;
        if let Some(path) = &options.write {
            if !path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION)
            }) {
                return Err("emitted scripts must use the .phont extension".to_owned());
            }
            fs::write(path, source)
                .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            if !options.quiet {
                println!("wrote {}", path.display());
            }
        } else {
            print!("{source}");
        }
        return Ok(());
    }
    let script_path = options.script.as_ref().ok_or_else(|| HELP.to_owned())?;
    if script_path.as_os_str() != "-"
        && !script_path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION))
    {
        return Err("script must use the .phont extension, or '-' for standard input".to_owned());
    }
    let source = if options.module_root.is_some() {
        None
    } else if script_path.as_os_str() == "-" {
        let mut source = String::new();
        io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("could not read standard input: {error}"))?;
        Some(source)
    } else {
        Some(
            fs::read_to_string(script_path)
                .map_err(|error| format!("could not read {}: {error}", script_path.display()))?,
        )
    };
    let source_name = if script_path.as_os_str() == "-" {
        "<stdin>".to_owned()
    } else {
        script_path.display().to_string()
    };
    if options.check {
        let diagnostics = if let Some(module_root) = &options.module_root {
            phonoscript_runtime::check_file(script_path, module_root)
        } else {
            phonoscript_runtime::check_named(
                &source_name,
                source.as_deref().expect("source-only check has source"),
            )
        };
        let has_errors = diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error);
        if options.json {
            let rendered = diagnostics.iter().map(json_diagnostic).collect::<Vec<_>>();
            println!(
                "{}",
                serde_json::to_string(&JsonCheckReport {
                    status: if has_errors { "error" } else { "ok" },
                    language: "PhonoScript",
                    language_version: phonoscript_runtime::LANGUAGE_VERSION,
                    source: &source_name,
                    diagnostics: rendered,
                })
                .map_err(|error| format!("could not encode JSON report: {error}"))?
            );
        } else {
            for diagnostic in &diagnostics {
                print_diagnostic(diagnostic);
            }
            if !options.quiet && !has_errors {
                println!("PhonoScript check passed");
            }
        }
        return if has_errors {
            Err(String::new())
        } else {
            Ok(())
        };
    }
    let base = match options.base {
        Some(path) => document::load(&path)?,
        None => ConvalgenDocument::blank(),
    };
    let result = if let Some(module_root) = &options.module_root {
        phonoscript_runtime::run_file(script_path, module_root, &base)
    } else {
        phonoscript_runtime::run_named(
            &source_name,
            source.as_deref().expect("source-only execution has source"),
            &base,
        )
    };
    if options.json {
        let report = json_report(&source_name, &result);
        println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|error| format!("could not encode JSON report: {error}"))?
        );
    } else {
        if !options.quiet {
            for line in &result.standard_output {
                println!("{line}");
            }
        }
        for diagnostic in &result.diagnostics {
            print_diagnostic(diagnostic);
        }
    }
    if !result.succeeded() {
        return Err(String::new());
    }
    if let Some(path) = options.write {
        let destination = document::save(Path::new(&path), &result.document)?;
        if !options.json && !options.quiet {
            println!("saved {}", destination.display());
        }
    }
    Ok(())
}

fn selected_tableau(value: SelectedTableau) -> String {
    match value {
        SelectedTableau::Source => "source".to_owned(),
        SelectedTableau::Target => "target".to_owned(),
        SelectedTableau::Dataset(index) => format!("dataset[{index}]"),
    }
}

fn json_report<'a>(source_name: &'a str, result: &'a RunResult) -> JsonReport<'a> {
    let diagnostics = result.diagnostics.iter().map(json_diagnostic).collect();
    let approximate_boundaries = result
        .boundary_conversions
        .iter()
        .map(|boundary| JsonBoundary {
            coordinate: &boundary.coordinate,
            exact_value: boundary.exact_value.to_string(),
            engine_value: boundary.engine_value,
            line: boundary.span.start.line,
            column: boundary.span.start.column,
        })
        .collect();
    JsonReport {
        status: if result.succeeded() { "ok" } else { "error" },
        language: "PhonoScript",
        language_version: phonoscript_runtime::LANGUAGE_VERSION,
        source: source_name,
        selected_tableau: selected_tableau(result.selected_tableau),
        output: &result.standard_output,
        value: json_value(&result.value),
        rendered_value: result.value.render(),
        diagnostics,
        approximate_boundaries,
        statistics: JsonStatistics {
            modules_loaded: result.statistics.modules_loaded,
            steps: result.statistics.steps,
            statements: result.statistics.statements,
            expressions: result.statistics.expressions,
            calls: result.statistics.calls,
            loop_iterations: result.statistics.loop_iterations,
            engine_calls: result.statistics.engine_calls,
            exact_to_engine_conversions: result.statistics.exact_to_engine_conversions,
            queued_file_effects: result.statistics.queued_file_effects,
        },
        document: &result.document,
    }
}

fn json_diagnostic(diagnostic: &phonoscript_runtime::RuntimeDiagnostic) -> JsonDiagnostic<'_> {
    JsonDiagnostic {
        source: &diagnostic.source_name,
        code: &diagnostic.code,
        severity: match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        message: &diagnostic.message,
        primary: json_span(diagnostic.primary),
        related: diagnostic
            .related
            .iter()
            .map(|related| JsonRelatedSpan {
                source: &diagnostic.source_name,
                span: json_span(related.span),
                message: &related.message,
            })
            .collect(),
        call_stack: diagnostic
            .call_stack
            .iter()
            .map(|frame| JsonCallFrame {
                function: &frame.function,
                source: &frame.source_name,
                span: json_span(frame.span),
            })
            .collect(),
        line: diagnostic.primary.start.line,
        column: diagnostic.primary.start.column,
        help: diagnostic.help.as_deref(),
    }
}

fn json_span(span: Span) -> JsonSpan {
    JsonSpan {
        start: JsonPosition {
            byte: span.start.byte,
            line: span.start.line,
            column: span.start.column,
        },
        end: JsonPosition {
            byte: span.end.byte,
            line: span.end.line,
            column: span.end.column,
        },
    }
}

fn json_value(value: &Value) -> JsonValue {
    match value {
        Value::Number(Number::Exact(value)) => JsonValue::ExactNumber {
            numerator: value.numer().to_string(),
            denominator: value.denom().to_string(),
        },
        Value::Number(Number::Approximate(value)) => JsonValue::ApproximateNumber { value: *value },
        Value::Boolean(value) => JsonValue::Boolean { value: *value },
        Value::Text(value) => JsonValue::Text {
            value: value.clone(),
        },
        Value::List(values) => JsonValue::List {
            items: values.iter().map(json_value).collect(),
        },
        Value::Record(values) => JsonValue::Record {
            fields: values
                .iter()
                .map(|(key, value)| (key.clone(), json_value(value)))
                .collect(),
        },
        Value::Null => JsonValue::Null,
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn diagnostic_lines(diagnostic: &RuntimeDiagnostic) -> Vec<String> {
    let mut lines = vec![format!(
        "{}:{}:{}: {}[{}]: {}",
        diagnostic.source_name,
        diagnostic.primary.start.line,
        diagnostic.primary.start.column,
        severity_name(diagnostic.severity),
        diagnostic.code,
        diagnostic.message
    )];
    lines.extend(diagnostic.related.iter().map(|related| {
        format!(
            "  related: {}:{}:{}-{}:{}: {}",
            diagnostic.source_name,
            related.span.start.line,
            related.span.start.column,
            related.span.end.line,
            related.span.end.column,
            related.message
        )
    }));
    if let Some(help) = &diagnostic.help {
        lines.push(format!("  help: {help}"));
    }
    lines.extend(diagnostic.call_stack.iter().rev().map(|frame| {
        format!(
            "  at {} ({}:{}:{})",
            frame.function, frame.source_name, frame.span.start.line, frame.span.start.column
        )
    }));
    lines
}

fn print_diagnostic(diagnostic: &RuntimeDiagnostic) {
    for line in diagnostic_lines(diagnostic) {
        eprintln!("{line}");
    }
}

fn parse_arguments(arguments: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--base" => {
                options.base = Some(PathBuf::from(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| "--base requires a path".to_owned())?,
                ));
                index += 2;
            }
            "--module-root" => {
                options.module_root = Some(PathBuf::from(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| "--module-root requires a path".to_owned())?,
                ));
                index += 2;
            }
            "--write" => {
                options.write = Some(PathBuf::from(
                    arguments
                        .get(index + 1)
                        .ok_or_else(|| "--write requires a path".to_owned())?,
                ));
                index += 2;
            }
            "--emit" => {
                options.emit =
                    Some(PathBuf::from(arguments.get(index + 1).ok_or_else(
                        || "--emit requires a project path".to_owned(),
                    )?));
                index += 2;
            }
            "--check" => {
                options.check = true;
                index += 1;
            }
            "--json" => {
                options.json = true;
                index += 1;
            }
            "--quiet" => {
                options.quiet = true;
                index += 1;
            }
            value if value.starts_with('-') && value != "-" => {
                return Err(format!("unknown option {value}"));
            }
            value if options.script.is_none() => {
                options.script = Some(PathBuf::from(value));
                index += 1;
            }
            value if options.base.is_none() => {
                options.base = Some(PathBuf::from(value));
                index += 1;
            }
            value => return Err(format!("unexpected argument {value}")),
        }
    }
    if options.json && options.quiet {
        return Err("--json and --quiet are mutually exclusive".to_owned());
    }
    if options.emit.is_some() && options.script.is_some() {
        return Err("--emit does not accept a script input".to_owned());
    }
    if options.check && options.emit.is_some() {
        return Err("--check and --emit are mutually exclusive".to_owned());
    }
    if options.check && options.write.is_some() {
        return Err("--check does not write a project".to_owned());
    }
    if options.emit.is_some() && options.module_root.is_some() {
        return Err("--emit does not use a module root".to_owned());
    }
    if options.script.as_deref() == Some(Path::new("-")) && options.module_root.is_some() {
        return Err(
            "--module-root requires a file entry; standard input has no import directory"
                .to_owned(),
        );
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_bigint::BigInt;
    use num_rational::BigRational;

    #[test]
    fn command_line_accepts_stdin_and_explicit_paths() {
        let options = parse_arguments(&[
            "--json".to_owned(),
            "--base".to_owned(),
            "base.ottab".to_owned(),
            "-".to_owned(),
            "--write".to_owned(),
            "result.ottab".to_owned(),
        ])
        .expect("valid arguments");
        assert!(options.json);
        assert_eq!(options.script, Some(PathBuf::from("-")));
        assert_eq!(options.base, Some(PathBuf::from("base.ottab")));
        assert_eq!(options.write, Some(PathBuf::from("result.ottab")));
    }

    #[test]
    fn command_line_accepts_an_explicit_module_root_only_for_file_execution() {
        let options = parse_arguments(&[
            "--module-root".to_owned(),
            "project".to_owned(),
            "project/main.phont".to_owned(),
        ])
        .expect("valid module arguments");
        assert_eq!(options.module_root, Some(PathBuf::from("project")));
        assert_eq!(options.script, Some(PathBuf::from("project/main.phont")));

        let stdin = parse_arguments(&[
            "--module-root".to_owned(),
            "project".to_owned(),
            "-".to_owned(),
        ])
        .expect_err("standard input has no importer path");
        assert!(stdin.contains("requires a file entry"));

        let emit = parse_arguments(&[
            "--module-root".to_owned(),
            "project".to_owned(),
            "--emit".to_owned(),
            "analysis.ottab".to_owned(),
        ])
        .expect_err("emission does not resolve modules");
        assert!(emit.contains("does not use a module root"));
    }

    #[test]
    fn json_report_encodes_exact_and_approximate_values_structurally() {
        let result =
            phonoscript_runtime::run_named("ratio.phont", "1/3\n", &ConvalgenDocument::blank());
        assert!(result.succeeded());
        let report =
            serde_json::to_value(json_report("ratio.phont", &result)).expect("serializable report");
        assert_eq!(report["value"]["type"], "exact_number");
        assert_eq!(report["value"]["numerator"], "1");
        assert_eq!(report["value"]["denominator"], "3");
        assert_eq!(report["rendered_value"], "1/3");

        let approximate =
            serde_json::to_value(json_value(&Value::Number(Number::Approximate(0.125))))
                .expect("serializable approximation");
        assert_eq!(approximate["type"], "approximate_number");
        assert_eq!(approximate["value"], 0.125);

        let negative = json_value(&Value::Number(Number::Exact(BigRational::new(
            BigInt::from(-2),
            BigInt::from(5),
        ))));
        assert_eq!(
            negative,
            JsonValue::ExactNumber {
                numerator: "-2".to_owned(),
                denominator: "5".to_owned(),
            }
        );
    }

    #[test]
    fn json_and_text_diagnostics_retain_sources_spans_related_notes_and_frames() {
        let parsed = phonoscript_runtime::check_named("unclosed.phont", "{\n");
        let diagnostic = parsed
            .iter()
            .find(|diagnostic| diagnostic.code == "PSP0106")
            .expect("unclosed block diagnostic");
        let encoded =
            serde_json::to_value(json_diagnostic(diagnostic)).expect("serializable diagnostic");
        assert_eq!(encoded["source"], "unclosed.phont");
        assert_eq!(encoded["primary"]["start"]["line"], 2);
        assert_eq!(encoded["related"][0]["span"]["start"]["line"], 1);
        assert_eq!(encoded["related"][0]["message"], "this block starts here");

        let failed = phonoscript_runtime::run_named(
            "nested.phont",
            "fn inner(n) { return n / 0 }\nfn outer(n) { return inner(n) }\nouter(1)\n",
            &ConvalgenDocument::blank(),
        );
        let diagnostic = failed
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "PSR0402")
            .expect("division-by-zero diagnostic");
        let encoded = serde_json::to_value(json_diagnostic(diagnostic))
            .expect("serializable runtime diagnostic");
        assert_eq!(encoded["call_stack"][0]["function"], "outer");
        assert_eq!(encoded["call_stack"][1]["function"], "inner");
        assert_eq!(encoded["call_stack"][1]["source"], "nested.phont");

        let lines = diagnostic_lines(diagnostic);
        assert!(lines[0].starts_with("nested.phont:"));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("at inner (nested.phont:"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("at outer (nested.phont:"))
        );
    }
}
