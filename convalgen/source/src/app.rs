use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, FontId, Key, Layout, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use egui_extras::{Column, TableBuilder};

use crate::document;
use crate::engine::{CloneAuditResult, ComparisonStatus, ExactRankingShare};
use crate::exact::NumericScalar;
use crate::export::{self, ExportFormat};
use crate::learning::ranking_implications;
use crate::model::{
    Candidate, ComparisonMode, Constraint, ConsumerMode, ConvalgenDocument, EvaluatorKind,
    MAX_VIOLATION, NormalizerPolicy, PlotKind, QueryKind, ResponseDomain, SerialMove, Tableau,
    TiePolicy, UNSET_VIOLATION, next_stable_id,
};
use crate::otsoft;
use crate::phonological_engine::PhonologicalEngine;
use crate::phonoscript_editor;
use crate::phonoscript_frontend::{self, Severity, TokenTag};
use crate::phonoscript_runtime::{
    self, RunResult, RuntimeDiagnostic, RuntimeDiagnosticCode, SelectedTableau,
};
use crate::theme::{self, CAUTION, FOCUS, INK, LINE, MUTED, NEGATIVE, PANEL, SURFACE};

const NAVIGATOR_BREAKPOINT: f32 = 900.0;
const INSPECTOR_BREAKPOINT: f32 = 900.0;
const CONSOLE_BREAKPOINT: f32 = 610.0;
const TOOLBAR_BREAKPOINT: f32 = 920.0;
const UNTITLED_PHONT_SOURCE: &str = "Untitled.phont";

fn phont_source_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| UNTITLED_PHONT_SOURCE.to_owned())
}

fn confined_phont_module_root(entry_path: &Path) -> &Path {
    entry_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn first_import_span(source: &str) -> Option<phonoscript_frontend::Span> {
    phonoscript_frontend::parse(source)
        .tokens
        .into_iter()
        .find(|token| token.kind.tag() == TokenTag::Import)
        .map(|token| token.span)
}

fn check_phont_source(source: &str, path: Option<&Path>, dirty: bool) -> Vec<RuntimeDiagnostic> {
    if let Some(path) = path.filter(|_| !dirty) {
        return phonoscript_runtime::check_file(path, confined_phont_module_root(path));
    }

    let source_name = phont_source_name(path);
    let mut diagnostics = phonoscript_runtime::check_named(&source_name, source);
    let Some(import_span) = first_import_span(source) else {
        return diagnostics;
    };
    let (message, help) = if path.is_some() {
        (
            "imports cannot be resolved from unsaved editor changes",
            "Save the current .phont file before checking or running its module graph.",
        )
    } else {
        (
            "imports require a saved PhonoScript entry file",
            "Save this script as a .phont file. ConvalGEN confines module resolution to that file's containing directory.",
        )
    };
    let mut replaced = false;
    for diagnostic in &mut diagnostics {
        if diagnostic.code == RuntimeDiagnosticCode::ModuleResolution.as_str()
            && diagnostic.source_name == source_name
            && diagnostic
                .message
                .contains("imports require file execution")
        {
            diagnostic.message = message.to_owned();
            diagnostic.help = Some(help.to_owned());
            replaced = true;
        }
    }
    if !replaced {
        diagnostics.push(RuntimeDiagnostic {
            source_name,
            code: RuntimeDiagnosticCode::ModuleResolution.as_str().to_owned(),
            severity: Severity::Error,
            message: message.to_owned(),
            primary: import_span,
            related: Vec::new(),
            help: Some(help.to_owned()),
            call_stack: Vec::new(),
        });
    }
    diagnostics
}

fn run_phont_source(
    source: &str,
    path: Option<&Path>,
    dirty: bool,
    initial: &ConvalgenDocument,
) -> Result<RunResult, Vec<RuntimeDiagnostic>> {
    let diagnostics = check_phont_source(source, path, dirty);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(diagnostics);
    }
    if let Some(path) = path.filter(|_| !dirty) {
        Ok(phonoscript_runtime::run_file(
            path,
            confined_phont_module_root(path),
            initial,
        ))
    } else {
        Ok(phonoscript_runtime::run_named(
            &phont_source_name(path),
            source,
            initial,
        ))
    }
}

fn diagnostic_location(diagnostic: &RuntimeDiagnostic) -> String {
    format!(
        "{}:{}:{}",
        diagnostic.source_name, diagnostic.primary.start.line, diagnostic.primary.start.column
    )
}

fn phont_path_event(action: &str, path: &Path) -> String {
    format!("{action} {}", path_display_name(path))
}

fn phonoscript_source_editor(
    ui: &mut egui::Ui,
    source: &mut String,
    editor_height: f32,
    diagnostic_spans: &[phonoscript_editor::EditorDiagnosticSpan],
) -> egui::scroll_area::ScrollAreaOutput<egui::Response> {
    let viewport_width = ui.available_width().max(1.0);
    let font_id = phonoscript_editor::source_font_id();
    let content_width = ui.fonts_mut(|fonts| {
        phonoscript_editor::source_editor_content_width(source, viewport_width, |character| {
            fonts.glyph_width(&font_id, character)
        })
    });
    let desired_text_width = (content_width - 8.0).max(24.0);
    let dark = ui.visuals().dark_mode;
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, wrap_width: f32| {
        let job = phonoscript_editor::layout_job_with_diagnostics(
            buffer.as_str(),
            wrap_width,
            dark,
            diagnostic_spans,
        );
        ui.fonts_mut(|fonts| fonts.layout_job(job))
    };

    egui::ScrollArea::both()
        .id_salt("phonoscript-source-scroll")
        .auto_shrink([false, false])
        .max_width(viewport_width)
        .max_height(editor_height)
        .min_scrolled_width(viewport_width)
        .min_scrolled_height(editor_height)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.add_sized(
                [content_width, editor_height],
                egui::TextEdit::multiline(source)
                    .id_salt("phonoscript-source-editor")
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(desired_text_width)
                    .hint_text("Write PhonoScript declarations and commands here")
                    .layouter(&mut layouter),
            )
        })
}

#[derive(Clone)]
struct ScalarEditState {
    text: String,
    committed: String,
    error: Option<String>,
}

fn violation_editor(ui: &mut egui::Ui, mark: &mut u16) -> egui::Response {
    let unset = *mark == UNSET_VIOLATION;
    let response = ui
        .scope(|ui| {
            if unset {
                ui.visuals_mut().override_text_color = Some(CAUTION);
            }
            ui.add(
                egui::DragValue::new(mark)
                    .range(0..=UNSET_VIOLATION)
                    .custom_formatter(|value, _| {
                        let value = value.round().clamp(0.0, f64::from(UNSET_VIOLATION)) as u16;
                        if value == UNSET_VIOLATION {
                            "—".to_owned()
                        } else {
                            value.to_string()
                        }
                    })
                    .custom_parser(|text| {
                        let text = text.trim();
                        if text.is_empty() || text == "—" {
                            Some(f64::from(UNSET_VIOLATION))
                        } else {
                            text.parse::<u16>()
                                .ok()
                                .filter(|value| *value <= MAX_VIOLATION)
                                .map(f64::from)
                        }
                    }),
            )
        })
        .inner;
    if unset {
        response.on_hover_text(
            "Unset: the phonologist must enter this violation count before evaluation",
        )
    } else {
        response
    }
}

fn drag_scalar(
    ui: &mut egui::Ui,
    scalar: &mut NumericScalar,
    range: std::ops::RangeInclusive<f64>,
    _speed: f64,
) -> bool {
    let widget_id = ui.next_auto_id();
    let state_id = widget_id.with("numeric-scalar-source");
    let canonical = scalar.canonical();
    let has_focus = ui.memory(|memory| memory.has_focus(widget_id));
    let mut state = ui
        .data_mut(|data| data.get_temp::<ScalarEditState>(state_id))
        .unwrap_or_else(|| ScalarEditState {
            text: canonical.clone(),
            committed: canonical.clone(),
            error: None,
        });
    if !has_focus && state.committed != canonical {
        state.text.clone_from(&canonical);
        state.committed.clone_from(&canonical);
        state.error = None;
    }

    let response = ui.add(
        egui::TextEdit::singleline(&mut state.text)
            .id(widget_id)
            .desired_width(68.0)
            .font(egui::TextStyle::Monospace),
    );
    let mut changed = false;
    if response.changed() {
        let parsed = NumericScalar::parse_editor(&state.text)
            .map_err(|error| error.to_string())
            .and_then(|parsed| {
                let center = parsed.to_f64_center().map_err(|error| error.to_string())?;
                if range.contains(&center) {
                    Ok(parsed)
                } else {
                    Err("value lies outside the admitted editor range".to_owned())
                }
            });
        match parsed {
            Ok(parsed) => {
                *scalar = parsed;
                state.committed = scalar.canonical();
                state.error = None;
                changed = true;
            }
            Err(error) => state.error = Some(error.to_string()),
        }
    }
    if response.lost_focus() && state.error.is_some() {
        state.text = scalar.canonical();
        state.committed.clone_from(&state.text);
        state.error = None;
    }
    if let Some(error) = &state.error {
        ui.painter().rect_stroke(
            response.rect.expand(1.0),
            2.0,
            Stroke::new(1.0_f32, NEGATIVE),
            StrokeKind::Inside,
        );
        response.on_hover_text(format!(
            "{error}\nExact values accept integers, decimals, or fractions. Prefix an exploratory approximation with ~."
        ));
    } else {
        response.on_hover_text(
            "Exact values accept integers, decimals, or fractions. Prefix an exploratory approximation with ~.",
        );
    }
    ui.data_mut(|data| data.insert_temp(state_id, state));
    changed
}

fn drag_optional_scalar(
    ui: &mut egui::Ui,
    scalar: &mut Option<NumericScalar>,
    range: std::ops::RangeInclusive<f64>,
    speed: f64,
) -> bool {
    if let Some(scalar) = scalar {
        drag_scalar(ui, scalar, range, speed)
    } else {
        ui.label(RichText::new("unavailable").color(CAUTION));
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workspace {
    Project,
    Tableau,
    Serial,
    SecondOrder,
    Diagnostics,
    QCalculus,
    Plots,
    PhonoScript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViolationCell {
    candidate: usize,
    constraint: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ShortcutProfile {
    #[default]
    Standard,
    Laptop,
    Disabled,
}

impl ShortcutProfile {
    const ALL: [Self; 3] = [Self::Standard, Self::Laptop, Self::Disabled];

    const fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Laptop => "Laptop-friendly",
            Self::Disabled => "Editing shortcuts off",
        }
    }
}

impl Workspace {
    const ALL: [Self; 8] = [
        Self::Project,
        Self::Tableau,
        Self::Serial,
        Self::SecondOrder,
        Self::Diagnostics,
        Self::QCalculus,
        Self::Plots,
        Self::PhonoScript,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Tableau => "Tableau editor",
            Self::Serial => "Serial derivation",
            Self::SecondOrder => "Second-Order Tableau",
            Self::Diagnostics => "Ranking and diagnostics",
            Self::QCalculus => "Q-Calculus audit",
            Self::Plots => "Plots",
            Self::PhonoScript => "PhonoScript script",
        }
    }

    const TOOLS: [(Self, &'static str); 6] = [
        (Self::Serial, "Serial derivation"),
        (Self::SecondOrder, "Second-Order Tableau"),
        (Self::Diagnostics, "Ranking and diagnostics"),
        (Self::QCalculus, "Q-Calculus audit"),
        (Self::Plots, "Plots"),
        (Self::PhonoScript, "PhonoScript script"),
    ];
}

pub struct ConvalgenApp {
    document: ConvalgenDocument,
    path: Option<PathBuf>,
    dirty: bool,
    workspace: Workspace,
    active_tableau: usize,
    selected_candidate: usize,
    selected_constraint: usize,
    selected_violation: Option<ViolationCell>,
    edit_target: bool,
    row_filter: String,
    status: String,
    console: Vec<String>,
    command: String,
    show_navigator: bool,
    show_inspector: bool,
    show_console: bool,
    show_about: bool,
    show_help: bool,
    show_preferences: bool,
    shortcut_profile: ShortcutProfile,
    export_format: ExportFormat,
    last_evaluation: Duration,
    diagnostics: Vec<String>,
    phont_source: String,
    phont_path: Option<PathBuf>,
    phont_output: Vec<String>,
    phont_dirty: bool,
    phont_diagnostics: Vec<RuntimeDiagnostic>,
    undo_stack: Vec<ConvalgenDocument>,
    redo_stack: Vec<ConvalgenDocument>,
    history_replaying: bool,
    last_window_title: String,
}

impl ConvalgenApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        theme::install(&context.egui_ctx, false);
        Self {
            document: ConvalgenDocument::blank(),
            path: None,
            dirty: false,
            workspace: Workspace::Tableau,
            active_tableau: 0,
            selected_candidate: 0,
            selected_constraint: 0,
            selected_violation: None,
            edit_target: false,
            row_filter: String::new(),
            status: "Ready".to_owned(),
            console: vec![
                "ConvalGEN command interface ready. Type help to list commands.".to_owned(),
            ],
            command: String::new(),
            show_navigator: true,
            show_inspector: true,
            show_console: false,
            show_about: false,
            show_help: false,
            show_preferences: false,
            shortcut_profile: ShortcutProfile::Standard,
            export_format: ExportFormat::Svg,
            last_evaluation: Duration::ZERO,
            diagnostics: Vec::new(),
            phont_source: String::new(),
            phont_path: None,
            phont_output: Vec::new(),
            phont_dirty: false,
            phont_diagnostics: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            history_replaying: false,
            last_window_title: String::new(),
        }
    }

    pub fn new_with_path(context: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        let mut application = Self::new(context);
        if let Some(path) = path {
            application.open_path(&path);
        }
        application
    }

    fn display_name(&self) -> String {
        let suffix = if self.dirty { " — Edited" } else { "" };
        format!("{}{}", self.document.title, suffix)
    }

    fn active(&self) -> &Tableau {
        self.document
            .dataset
            .get(self.active_tableau)
            .unwrap_or(&self.document.source)
    }

    fn active_mut(&mut self) -> &mut Tableau {
        if self.document.dataset.is_empty() {
            self.document.dataset.push(self.document.source.clone());
        }
        self.active_tableau = self
            .active_tableau
            .min(self.document.dataset.len().saturating_sub(1));
        &mut self.document.dataset[self.active_tableau]
    }

    fn active_label(&self) -> String {
        let tableau = self.active();
        if !tableau.name.trim().is_empty() {
            tableau.name.clone()
        } else if !tableau.input.trim().is_empty() {
            tableau.input.clone()
        } else {
            format!("Tableau {}", self.active_tableau + 1)
        }
    }

    fn active_evaluator(&self) -> EvaluatorKind {
        self.active().evaluator_or(self.document.evaluator)
    }

    fn active_temperature(&self) -> f64 {
        self.active().temperature_or(&self.document.temperature)
    }

    fn mark_changed(&mut self) {
        self.dirty = true;
        self.status = "Analysis updated; all derived results will be recalculated.".to_owned();
    }

    fn add_tableau(&mut self) {
        let mut tableau = ConvalgenDocument::blank().dataset.remove(0);
        tableau.id = next_stable_id(
            "tableau",
            self.document
                .dataset
                .iter()
                .map(|tableau| tableau.id.as_str()),
        );
        tableau.name = format!("Tableau {}", self.document.dataset.len() + 1);
        if let Some(active) = self.document.dataset.get(self.active_tableau) {
            tableau.constraints.clone_from(&active.constraints);
            for candidate in &mut tableau.candidates {
                candidate.violations = vec![UNSET_VIOLATION; tableau.constraints.len()];
            }
            tableau.normalize();
        }
        self.document.dataset.push(tableau);
        self.active_tableau = self.document.dataset.len() - 1;
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.workspace = Workspace::Tableau;
        self.mark_changed();
        self.status =
            "New tableau created; enter every unset violation count before evaluation.".to_owned();
    }

    fn duplicate_tableau(&mut self) {
        let mut tableau = self.active().clone();
        let base = self.active_label();
        tableau.id = next_stable_id(
            "tableau",
            self.document
                .dataset
                .iter()
                .map(|tableau| tableau.id.as_str()),
        );
        tableau.name = format!("{base} copy");
        self.document.dataset.push(tableau);
        self.active_tableau = self.document.dataset.len() - 1;
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.workspace = Workspace::Tableau;
        self.mark_changed();
    }

    fn remove_tableau(&mut self) {
        if self.document.dataset.len() <= 1 {
            self.report_error("a project must retain at least one tableau".to_owned());
            return;
        }
        let index = self
            .active_tableau
            .min(self.document.dataset.len().saturating_sub(1));
        self.document.dataset.remove(index);
        self.active_tableau = index.min(self.document.dataset.len() - 1);
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.mark_changed();
    }

    fn move_tableau(&mut self, direction: isize) {
        let Some(destination) = self.active_tableau.checked_add_signed(direction) else {
            return;
        };
        if destination >= self.document.dataset.len() {
            return;
        }
        self.document.dataset.swap(self.active_tableau, destination);
        self.active_tableau = destination;
        self.mark_changed();
    }

    fn move_candidate(&mut self, direction: isize) {
        let count = self.active().candidates.len();
        let Some(destination) = self.selected_candidate.checked_add_signed(direction) else {
            return;
        };
        if destination >= count {
            return;
        }
        let source = self.selected_candidate;
        self.active_mut().candidates.swap(source, destination);
        self.selected_candidate = destination;
        self.selected_violation = None;
        self.mark_changed();
    }

    fn move_constraint(&mut self, direction: isize) {
        let count = self.active().constraints.len();
        let Some(destination) = self.selected_constraint.checked_add_signed(direction) else {
            return;
        };
        if destination >= count {
            return;
        }
        let rerank = self.active_evaluator() == EvaluatorKind::Ot;
        let source = self.selected_constraint;
        let tableau = self.active_mut();
        tableau.constraints.swap(source, destination);
        for candidate in &mut tableau.candidates {
            candidate.violations.swap(source, destination);
        }
        if rerank {
            assign_strict_ranks_by_column(tableau);
        }
        self.selected_constraint = destination;
        self.selected_violation = None;
        self.mark_changed();
        self.status = if rerank {
            "Constraint moved and strict OT ranking updated to match column order.".to_owned()
        } else {
            "Constraint column moved; weighted evaluation remains controlled by its weight."
                .to_owned()
        };
    }

    fn tie_constraint_left(&mut self) {
        if self.active_evaluator() != EvaluatorKind::Ot || self.selected_constraint == 0 {
            return;
        }
        let index = self.selected_constraint;
        let tableau = self.active_mut();
        let stratum = tableau.constraints[index - 1].stratum;
        tableau.constraints[index].stratum = stratum;
        compact_constraint_strata(tableau);
        self.mark_changed();
        self.status = "Selected constraint tied with the constraint to its left.".to_owned();
    }

    fn make_constraint_order_strict(&mut self) {
        if self.active_evaluator() != EvaluatorKind::Ot {
            return;
        }
        assign_strict_ranks_by_column(self.active_mut());
        self.mark_changed();
        self.status = "Constraint ties removed; column order is now the strict ranking.".to_owned();
    }

    fn set_tie_policy(&mut self, policy: TiePolicy) {
        self.active_mut().set_tie_policy(policy);
        self.mark_changed();
        self.status = format!("Tie policy: {}.", policy.label());
    }

    fn clear_selected_violation(&mut self) {
        let Some(cell) = self.selected_violation else {
            self.status = "Select an editable violation cell before pressing Delete.".to_owned();
            return;
        };
        let tableau = self.active_mut();
        let Some(candidate) = tableau.candidates.get_mut(cell.candidate) else {
            self.selected_violation = None;
            return;
        };
        let Some(value) = candidate.violations.get_mut(cell.constraint) else {
            self.selected_violation = None;
            return;
        };
        *value = UNSET_VIOLATION;
        self.mark_changed();
        self.status = "Selected violation cell marked unset.".to_owned();
    }

    fn use_active_as_source(&mut self) {
        self.document.source = self.active().clone();
        self.edit_target = false;
        self.workspace = Workspace::SecondOrder;
        self.mark_changed();
    }

    fn use_active_as_target(&mut self) {
        self.document.target = self.active().clone();
        self.edit_target = true;
        self.workspace = Workspace::SecondOrder;
        self.mark_changed();
    }

    fn apply_constraints_to_project(&mut self) {
        let constraints = self.active().constraints.clone();
        for tableau in &mut self.document.dataset {
            replace_constraint_register(tableau, &constraints);
            tableau.normalize();
        }
        self.mark_changed();
        self.status = format!(
            "Applied {} to {}; replacement cells are unset until the phonologist enters them",
            counted(constraints.len(), "constraint", "constraints"),
            counted(self.document.dataset.len(), "tableau", "tableaux")
        );
    }

    fn new_document(&mut self) {
        self.document = ConvalgenDocument::blank();
        self.path = None;
        self.dirty = false;
        self.active_tableau = 0;
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.workspace = Workspace::Tableau;
        self.status = "New analysis".to_owned();
        self.console
            .push("new · created an untitled analysis".to_owned());
    }

    fn open_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ConvalGEN analysis", &[document::EXTENSION])
            .add_filter("PhonoScript script", &[phonoscript_runtime::EXTENSION])
            .pick_file()
        {
            self.open_path(&path);
        }
    }

    fn open_path(&mut self, path: &Path) {
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION))
        {
            self.open_phont_path(path);
            return;
        }
        match document::load(path) {
            Ok(document) => {
                self.document = document;
                self.path = Some(path.to_owned());
                self.dirty = false;
                self.active_tableau = 0;
                self.selected_candidate = 0;
                self.selected_constraint = 0;
                self.selected_violation = None;
                self.status = format!("Opened {}", path_display_name(path));
                self.console.push(format!("open · {}", path.display()));
            }
            Err(error) => self.report_error(error),
        }
    }

    fn new_phont_script(&mut self) {
        self.phont_source.clear();
        self.phont_path = None;
        self.phont_output.clear();
        self.phont_diagnostics.clear();
        self.phont_dirty = false;
        self.workspace = Workspace::PhonoScript;
        self.status = "New PhonoScript script".to_owned();
    }

    fn refresh_phont_diagnostics(&mut self) {
        self.phont_diagnostics = check_phont_source(
            &self.phont_source,
            self.phont_path.as_deref(),
            self.phont_dirty,
        );
    }

    fn open_phont_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PhonoScript script", &[phonoscript_runtime::EXTENSION])
            .pick_file()
        {
            self.open_phont_path(&path);
        }
    }

    fn open_phont_path(&mut self, path: &Path) {
        match fs::read_to_string(path) {
            Ok(source) => {
                self.phont_source = source;
                self.phont_path = Some(path.to_owned());
                self.phont_dirty = false;
                self.refresh_phont_diagnostics();
                self.workspace = Workspace::PhonoScript;
                self.status = format!("Opened PhonoScript script {}", path_display_name(path));
                self.phont_output.push(phont_path_event("opened", path));
            }
            Err(error) => self.report_error(format!(
                "could not read PhonoScript script {}: {error}",
                path.display()
            )),
        }
    }

    fn save_phont(&mut self) {
        if let Some(path) = self.phont_path.clone() {
            self.save_phont_path(&path);
        } else {
            self.save_phont_as();
        }
    }

    fn save_phont_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PhonoScript script", &[phonoscript_runtime::EXTENSION])
            .set_file_name("analysis.phont")
            .save_file()
        {
            self.save_phont_path(&path);
        }
    }

    fn save_phont_path(&mut self, path: &Path) {
        let destination = if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION))
        {
            path.to_owned()
        } else {
            path.with_extension(phonoscript_runtime::EXTENSION)
        };
        match fs::write(&destination, &self.phont_source) {
            Ok(()) => {
                self.phont_path = Some(destination.clone());
                self.phont_dirty = false;
                self.refresh_phont_diagnostics();
                self.status = format!(
                    "Saved PhonoScript script {}",
                    path_display_name(&destination)
                );
                self.phont_output
                    .push(phont_path_event("saved", &destination));
            }
            Err(error) => self.report_error(format!(
                "could not write PhonoScript script {}: {error}",
                destination.display()
            )),
        }
    }

    fn run_phont(&mut self) {
        let started = Instant::now();
        let result = match run_phont_source(
            &self.phont_source,
            self.phont_path.as_deref(),
            self.phont_dirty,
            &self.document,
        ) {
            Ok(result) => result,
            Err(diagnostics) => {
                self.phont_diagnostics = diagnostics;
                self.status = "PhonoScript has errors; execution was not started.".to_owned();
                return;
            }
        };
        self.last_evaluation = started.elapsed();
        self.phont_diagnostics = result.diagnostics.clone();
        self.phont_output
            .extend(result.standard_output.iter().cloned());
        self.console.extend(result.standard_output.iter().cloned());
        for diagnostic in &result.diagnostics {
            let severity = match diagnostic.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let rendered = format!(
                "{severity} · {} [{}] {}",
                diagnostic.code,
                diagnostic_location(diagnostic),
                diagnostic.message
            );
            self.phont_output.push(rendered.clone());
            self.console.push(rendered);
            if let Some(help) = &diagnostic.help {
                self.phont_output.push(format!("help · {help}"));
            }
        }
        if result.succeeded() {
            self.document = result.document;
            match result.selected_tableau {
                SelectedTableau::Source => self.edit_target = false,
                SelectedTableau::Target => self.edit_target = true,
                SelectedTableau::Dataset(index) => {
                    self.active_tableau = index.min(self.document.dataset.len().saturating_sub(1));
                }
            }
            self.dirty = true;
            let boundary_note = if result.boundary_conversions.is_empty() {
                String::new()
            } else {
                format!(
                    " · {}",
                    counted(
                        result.boundary_conversions.len(),
                        "explicit numerical boundary conversion",
                        "explicit numerical boundary conversions"
                    )
                )
            };
            self.status = format!(
                "PhonoScript completed in {}{boundary_note}",
                speed(self.last_evaluation)
            );
        } else {
            self.status = format!(
                "PhonoScript stopped without changing the project after {}",
                speed(self.last_evaluation)
            );
        }
    }

    fn save(&mut self) {
        if let Some(path) = self.path.clone() {
            self.save_path(&path);
        } else {
            self.save_as();
        }
    }

    fn save_as(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("ConvalGEN analysis", &[document::EXTENSION])
            .set_file_name("analysis.ottab")
            .save_file()
        {
            self.save_path(&path);
        }
    }

    fn save_path(&mut self, path: &Path) {
        match document::save(path, &self.document) {
            Ok(destination) => {
                self.path = Some(destination.clone());
                self.dirty = false;
                self.status = format!("Saved {}", path_display_name(&destination));
                self.console
                    .push(format!("save · {}", destination.display()));
            }
            Err(error) => self.report_error(error),
        }
    }

    fn import_legacy_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("OTSoft / MaxEnt Grammar Tool", &["txt", "tsv"])
            .pick_file()
        {
            match fs::read_to_string(&path)
                .map_err(|error| error.to_string())
                .and_then(|text| otsoft::import_tsv(&text))
            {
                Ok(tableaus) => {
                    self.document.dataset = tableaus;
                    self.document.source = self.document.dataset[0].clone();
                    self.document.target = self.document.source.clone();
                    self.active_tableau = 0;
                    self.dirty = true;
                    self.status = format!("Imported {}", path_display_name(&path));
                    self.console.push(format!(
                        "import · {} from {}",
                        counted(self.document.dataset.len(), "tableau", "tableaux"),
                        path.display()
                    ));
                }
                Err(error) => self.report_error(error),
            }
        }
    }

    fn export_legacy_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Tab-delimited tableaux", &["txt", "tsv"])
            .set_file_name("tableaux.txt")
            .save_file()
        {
            match otsoft::export_tsv(&self.document.dataset).and_then(|text| {
                fs::write(&path, text)
                    .map_err(|error| format!("could not write {}: {error}", path.display()))
            }) {
                Ok(()) => {
                    self.status = format!("Exported {}", path_display_name(&path));
                    self.console
                        .push(format!("export legacy · {}", path.display()));
                }
                Err(error) => self.report_error(error),
            }
        }
    }

    fn export_dialog(&mut self, content: &str) {
        let default_name = format!(
            "{}.{}",
            if content == "plot" { "plot" } else { "tableau" },
            self.export_format.extension()
        );
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                self.export_format.label(),
                &[self.export_format.extension()],
            )
            .set_file_name(&default_name)
            .save_file()
        {
            self.export_path(content, &path);
        }
    }

    fn export_path(&mut self, content: &str, path: &Path) {
        let svg = match content {
            "plot" => {
                let mut document = self.document.clone();
                document.source = self.active().clone();
                document.evaluator = self.active_evaluator();
                document.temperature = NumericScalar::gui_approximate(self.active_temperature())
                    .expect("active temperature is finite");
                export::plot_svg(&document)
            }
            "second-order" => export::tableau_svg(&self.document, true),
            _ => {
                let mut document = self.document.clone();
                document.source = self.active().clone();
                document.evaluator = self.active_evaluator();
                document.temperature = NumericScalar::gui_approximate(self.active_temperature())
                    .expect("active temperature is finite");
                export::tableau_svg(&document, false)
            }
        };
        let svg = match svg {
            Ok(svg) => svg,
            Err(error) => {
                self.report_error(error);
                return;
            }
        };
        match export::write_with_scale(
            &svg,
            path,
            self.export_format,
            self.document.presentation.export_scale,
        ) {
            Ok(destination) => {
                self.status = format!("Exported {}", path_display_name(&destination));
                self.console
                    .push(format!("export {content} · {}", destination.display()));
            }
            Err(error) => self.report_error(error),
        }
    }

    fn export_project_dialog(&mut self) {
        if let Some(directory) = rfd::FileDialog::new().pick_folder() {
            let mut completed = 0;
            for (index, tableau) in self.document.dataset.iter().enumerate() {
                let mut document = self.document.clone();
                document.source = tableau.clone();
                document.evaluator = tableau.evaluator_or(self.document.evaluator);
                document.temperature = NumericScalar::gui_approximate(
                    tableau.temperature_or(&self.document.temperature),
                )
                .expect("tableau temperature is finite");
                let label = if tableau.name.trim().is_empty() {
                    format!("tableau-{}", index + 1)
                } else {
                    file_stem(&tableau.name)
                };
                let path = directory.join(format!(
                    "{:02}-{}.{}",
                    index + 1,
                    label,
                    self.export_format.extension()
                ));
                let svg = match export::tableau_svg(&document, false) {
                    Ok(svg) => svg,
                    Err(error) => {
                        self.report_error(error);
                        return;
                    }
                };
                if let Err(error) = export::write_with_scale(
                    &svg,
                    &path,
                    self.export_format,
                    self.document.presentation.export_scale,
                ) {
                    self.report_error(error);
                    return;
                }
                completed += 1;
            }
            self.status = format!(
                "Exported {} to {}",
                counted(completed, "tableau", "tableaux"),
                path_display_name(&directory)
            );
            self.console.push(format!(
                "export project · {} to {}",
                counted(completed, "tableau", "tableaux"),
                directory.display()
            ));
        }
    }

    fn report_error(&mut self, error: String) {
        self.status = format!("Error: {error}");
        self.console.push(format!("error · {error}"));
    }

    fn undo_document_change(&mut self) {
        let Some(previous) = self.undo_stack.pop() else {
            self.status = "Nothing to undo".to_owned();
            return;
        };
        let current = std::mem::replace(&mut self.document, previous);
        self.redo_stack.push(current);
        self.active_tableau = self
            .active_tableau
            .min(self.document.dataset.len().saturating_sub(1));
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.dirty = true;
        self.history_replaying = true;
        self.status = "Undid analysis change".to_owned();
    }

    fn redo_document_change(&mut self) {
        let Some(next) = self.redo_stack.pop() else {
            self.status = "Nothing to redo".to_owned();
            return;
        };
        let current = std::mem::replace(&mut self.document, next);
        self.undo_stack.push(current);
        self.active_tableau = self
            .active_tableau
            .min(self.document.dataset.len().saturating_sub(1));
        self.selected_candidate = 0;
        self.selected_constraint = 0;
        self.selected_violation = None;
        self.dirty = true;
        self.history_replaying = true;
        self.status = "Redid analysis change".to_owned();
    }

    fn shortcuts(&mut self, context: &egui::Context) {
        let (
            new,
            open,
            save,
            save_as,
            run,
            palette,
            add_candidate,
            add_constraint,
            duplicate_candidate,
            delete_cell,
            candidate_up,
            candidate_down,
            constraint_left,
            constraint_right,
            toggle_navigator,
            toggle_inspector,
            undo,
            redo,
        ) = context.input(|input| {
            let command = input.modifiers.command;
            let shift = input.modifiers.shift;
            let alt = input.modifiers.alt;
            let editing_enabled = self.shortcut_profile != ShortcutProfile::Disabled;
            let laptop = self.shortcut_profile == ShortcutProfile::Laptop;
            (
                command && input.key_pressed(Key::N),
                command && input.key_pressed(Key::O),
                command && input.key_pressed(Key::S),
                command && shift && input.key_pressed(Key::S),
                command && input.key_pressed(Key::R),
                command && shift && input.key_pressed(Key::P),
                editing_enabled
                    && ((laptop && command && shift && input.key_pressed(Key::A))
                        || (!laptop && command && !shift && input.key_pressed(Key::Enter))),
                editing_enabled
                    && ((laptop && command && shift && input.key_pressed(Key::C))
                        || (!laptop && command && shift && input.key_pressed(Key::Enter))),
                editing_enabled && command && input.key_pressed(Key::D),
                editing_enabled
                    && ((!laptop && input.key_pressed(Key::Delete))
                        || (laptop && command && shift && input.key_pressed(Key::Backspace))),
                editing_enabled && alt && input.key_pressed(Key::ArrowUp),
                editing_enabled && alt && input.key_pressed(Key::ArrowDown),
                editing_enabled && alt && input.key_pressed(Key::ArrowLeft),
                editing_enabled && alt && input.key_pressed(Key::ArrowRight),
                command && input.key_pressed(Key::B),
                command && input.key_pressed(Key::I),
                command && !shift && input.key_pressed(Key::Z),
                command && shift && input.key_pressed(Key::Z),
            )
        });
        let editing_text = context.wants_keyboard_input();
        if !editing_text {
            if undo {
                self.undo_document_change();
            } else if redo {
                self.redo_document_change();
            }
        }
        if new {
            self.new_document();
        }
        if open {
            self.open_dialog();
        }
        if save_as {
            if self.workspace == Workspace::PhonoScript {
                self.save_phont_as();
            } else {
                self.save_as();
            }
        } else if save {
            if self.workspace == Workspace::PhonoScript {
                self.save_phont();
            } else {
                self.save();
            }
        }
        if run {
            if self.workspace == Workspace::PhonoScript {
                self.run_phont();
            } else {
                self.evaluate_now();
            }
        }
        if palette {
            self.show_console = true;
            context.memory_mut(|memory| memory.request_focus(egui::Id::new("command-line")));
        }
        if self.workspace == Workspace::Tableau && !editing_text {
            if add_constraint {
                self.add_constraint();
            } else if add_candidate {
                self.add_candidate();
            }
            if duplicate_candidate {
                self.duplicate_candidate();
            }
            if delete_cell {
                self.clear_selected_violation();
            }
            if candidate_up {
                self.move_candidate(-1);
            }
            if candidate_down {
                self.move_candidate(1);
            }
            if constraint_left {
                self.move_constraint(-1);
            }
            if constraint_right {
                self.move_constraint(1);
            }
        }
        if toggle_navigator && !editing_text {
            self.show_navigator = !self.show_navigator;
        }
        if toggle_inspector && !editing_text {
            self.show_inspector = !self.show_inspector;
        }
    }

    fn menu_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("menu-bar")
            .exact_height(29.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(context, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("New analysis          Cmd+N").clicked() {
                            self.new_document();
                            ui.close();
                        }
                        if ui.button("Open…                 Cmd+O").clicked() {
                            self.open_dialog();
                            ui.close();
                        }
                        if ui.button("Save                  Cmd+S").clicked() {
                            self.save();
                            ui.close();
                        }
                        if ui.button("Save As…        Shift+Cmd+S").clicked() {
                            self.save_as();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("New PhonoScript script").clicked() {
                            self.new_phont_script();
                            ui.close();
                        }
                        if ui.button("Open PhonoScript script…").clicked() {
                            self.open_phont_dialog();
                            ui.close();
                        }
                        if ui.button("Save PhonoScript script").clicked() {
                            self.save_phont();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Import OTSoft / MaxEnt Tool…").clicked() {
                            self.import_legacy_dialog();
                            ui.close();
                        }
                        if ui.button("Export tab-delimited tableaux…").clicked() {
                            self.export_legacy_dialog();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            context.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        if ui
                            .add_enabled(
                                !self.undo_stack.is_empty(),
                                egui::Button::new("Undo                 Cmd+Z"),
                            )
                            .clicked()
                        {
                            self.undo_document_change();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                !self.redo_stack.is_empty(),
                                egui::Button::new("Redo           Shift+Cmd+Z"),
                            )
                            .clicked()
                        {
                            self.redo_document_change();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Add candidate          Cmd+Return").clicked() {
                            self.add_candidate();
                            ui.close();
                        }
                        if ui.button("Add constraint    Shift+Cmd+Return").clicked() {
                            self.add_constraint();
                            ui.close();
                        }
                        if ui.button("Duplicate candidate       Cmd+D").clicked() {
                            self.duplicate_candidate();
                            ui.close();
                        }
                        if ui.button("Clear selected violation       Del").clicked() {
                            self.clear_selected_violation();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Move candidate up       Option+Up").clicked() {
                            self.move_candidate(-1);
                            ui.close();
                        }
                        if ui.button("Move candidate down   Option+Down").clicked() {
                            self.move_candidate(1);
                            ui.close();
                        }
                        if ui.button("Move constraint left   Option+Left").clicked() {
                            self.move_constraint(-1);
                            ui.close();
                        }
                        if ui.button("Move constraint right Option+Right").clicked() {
                            self.move_constraint(1);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Remove selected candidate").clicked() {
                            self.remove_candidate();
                            ui.close();
                        }
                        if ui.button("Remove selected constraint").clicked() {
                            self.remove_constraint();
                            ui.close();
                        }
                    });
                    ui.menu_button("Project", |ui| {
                        if ui.button("Project overview").clicked() {
                            self.workspace = Workspace::Project;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("New tableau").clicked() {
                            self.add_tableau();
                            ui.close();
                        }
                        if ui.button("Duplicate tableau").clicked() {
                            self.duplicate_tableau();
                            ui.close();
                        }
                        if ui.button("Remove tableau").clicked() {
                            self.remove_tableau();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Move tableau up").clicked() {
                            self.move_tableau(-1);
                            ui.close();
                        }
                        if ui.button("Move tableau down").clicked() {
                            self.move_tableau(1);
                            ui.close();
                        }
                        if ui.button("Apply current constraints to project").clicked() {
                            self.apply_constraints_to_project();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Use as Second-Order source").clicked() {
                            self.use_active_as_source();
                            ui.close();
                        }
                        if ui.button("Use as Second-Order target").clicked() {
                            self.use_active_as_target();
                            ui.close();
                        }
                    });
                    ui.menu_button("Analysis", |ui| {
                        if ui.button("Evaluate             Cmd+R").clicked() {
                            self.evaluate_now();
                            ui.close();
                        }
                        if ui.button("Infer OT ranking").clicked() {
                            self.infer_ranking();
                            ui.close();
                        }
                        if ui.button("Learn MaxEnt weights").clicked() {
                            self.learn_weights();
                            ui.close();
                        }
                        if ui.button("Run diagnostics").clicked() {
                            self.run_diagnostics();
                            ui.close();
                        }
                        if ui.button("Compute factorial typology").clicked() {
                            self.compute_typology();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Run PhonoScript script").clicked() {
                            self.workspace = Workspace::PhonoScript;
                            self.run_phont();
                            ui.close();
                        }
                    });
                    ui.menu_button("View", |ui| {
                        ui.checkbox(
                            &mut self.show_navigator,
                            "Navigator                    Cmd+B",
                        );
                        ui.checkbox(
                            &mut self.show_inspector,
                            "Inspector                     Cmd+I",
                        );
                        ui.checkbox(&mut self.show_console, "Command console");
                        ui.separator();
                        ui.checkbox(
                            &mut self.document.presentation.compact_rows,
                            "Compact tableau rows",
                        );
                    });
                    ui.menu_button("Options", |ui| {
                        if ui.button("Preferences…").clicked() {
                            self.show_preferences = true;
                            ui.close();
                        }
                        ui.separator();
                        ui.label(RichText::new("Shortcut profile").strong());
                        for profile in ShortcutProfile::ALL {
                            if ui
                                .radio_value(&mut self.shortcut_profile, profile, profile.label())
                                .clicked()
                            {
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("Export", |ui| {
                        for format in ExportFormat::ALL {
                            ui.radio_value(&mut self.export_format, format, format.label());
                        }
                        ui.separator();
                        if ui.button("Current tableau…").clicked() {
                            self.export_dialog("tableau");
                            ui.close();
                        }
                        if ui.button("Second-Order Tableau…").clicked() {
                            self.export_dialog("second-order");
                            ui.close();
                        }
                        if ui.button("Current plot…").clicked() {
                            self.export_dialog("plot");
                            ui.close();
                        }
                        if ui.button("All project tableaux…").clicked() {
                            self.export_project_dialog();
                            ui.close();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("ConvalGEN Help").clicked() {
                            self.show_help = true;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Command reference").clicked() {
                            self.show_console = true;
                            self.command = "help".to_owned();
                            self.run_command();
                            ui.close();
                        }
                        if ui.button("PhonoScript language reference").clicked() {
                            self.workspace = Workspace::PhonoScript;
                            self.phont_output.extend(phonoscript_reference());
                            ui.close();
                        }
                        if ui.button("About ConvalGEN").clicked() {
                            self.show_about = true;
                            ui.close();
                        }
                    });
                });
            });
    }

    fn toolbar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .inner_margin(egui::Margin::symmetric(10, 7)),
            )
            .show(context, |ui| {
                let toolbar_width = ui.available_width();
                let compact = toolbar_width < TOOLBAR_BREAKPOINT;
                ui.horizontal_wrapped(|ui| {
                    if !compact {
                        ui.label(RichText::new("ConvalGEN").size(15.0).strong().color(INK));
                        ui.separator();
                    }
                    if toolbar_sized_button(
                        ui,
                        "Save",
                        "Save the current document",
                        if compact { 64.0 } else { 74.0 },
                    ) {
                        self.save();
                    }
                    if !compact {
                        ui.separator();
                    }
                    let labels: Vec<String> = self
                        .document
                        .dataset
                        .iter()
                        .enumerate()
                        .map(|(index, tableau)| {
                            if tableau.name.trim().is_empty() {
                                format!("Tableau {}", index + 1)
                            } else {
                                tableau.name.clone()
                            }
                        })
                        .collect();
                    let active_label = self.active_label();
                    let toolbar_tableau_label =
                        truncate(&active_label, if compact { 12 } else { 28 });
                    let tableau_response = egui::ComboBox::from_id_salt("toolbar-tableau")
                        .width(if compact { 96.0 } else { 170.0 })
                        .selected_text(toolbar_tableau_label)
                        .show_ui(ui, |ui| {
                            for (index, label) in labels.iter().enumerate() {
                                if ui
                                    .selectable_value(&mut self.active_tableau, index, label)
                                    .clicked()
                                {
                                    self.workspace = Workspace::Tableau;
                                }
                            }
                        })
                        .response;
                    tableau_response.on_hover_text(active_label);
                    if ui
                        .add_sized([28.0, 28.0], egui::Button::new("+"))
                        .on_hover_text("New tableau")
                        .clicked()
                    {
                        self.add_tableau();
                    }
                    let mut active_evaluator = self.active_evaluator();
                    egui::ComboBox::from_id_salt("toolbar-evaluator")
                        .width(if compact { 84.0 } else { 160.0 })
                        .selected_text(if compact {
                            active_evaluator.short_label()
                        } else {
                            active_evaluator.label()
                        })
                        .show_ui(ui, |ui| {
                            for evaluator in EvaluatorKind::ALL {
                                if ui
                                    .selectable_value(
                                        &mut active_evaluator,
                                        evaluator,
                                        evaluator.label(),
                                    )
                                    .changed()
                                {
                                    self.active_mut().evaluator = Some(active_evaluator);
                                    self.dirty = true;
                                }
                            }
                        });
                    if active_evaluator == EvaluatorKind::MaxEnt {
                        ui.label(if compact { "T" } else { "temperature" })
                            .on_hover_text("MaxEnt temperature");
                        let mut temperature = self
                            .active()
                            .temperature
                            .clone()
                            .unwrap_or_else(|| self.document.temperature.clone());
                        if drag_scalar(ui, &mut temperature, f64::MIN_POSITIVE..=1_000_000.0, 0.05)
                        {
                            self.active_mut().temperature = Some(temperature);
                            self.dirty = true;
                        }
                    }
                    if toolbar_sized_button(
                        ui,
                        "Evaluate",
                        "Recalculate the active analysis",
                        if compact { 68.0 } else { 74.0 },
                    ) {
                        self.evaluate_now();
                    }
                    if compact {
                        let workspace_label = truncate(self.workspace.label(), 16);
                        let workspace_response = egui::ComboBox::from_id_salt("toolbar-workspace")
                            .width(132.0)
                            .selected_text(workspace_label)
                            .show_ui(ui, |ui| {
                                for workspace in Workspace::ALL {
                                    ui.selectable_value(
                                        &mut self.workspace,
                                        workspace,
                                        workspace.label(),
                                    );
                                }
                            })
                            .response;
                        workspace_response.on_hover_text(self.workspace.label());
                    } else {
                        ui.label(
                            RichText::new(speed(self.last_evaluation))
                                .small()
                                .color(MUTED),
                        );
                        let display_name = self.display_name();
                        ui.add(
                            egui::Label::new(RichText::new(&display_name).small().color(MUTED))
                                .truncate(),
                        )
                        .on_hover_text(display_name);
                    }
                });
            });
    }

    fn navigator(&mut self, context: &egui::Context) {
        if !self.show_navigator {
            return;
        }
        egui::SidePanel::left("navigator")
            .resizable(true)
            .default_width(218.0)
            .width_range(180.0..=340.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .inner_margin(egui::Margin::symmetric(8, 9)),
            )
            .show(context, |ui| {
                ui.label(
                    RichText::new("ANALYSIS DOCUMENT")
                        .small()
                        .strong()
                        .color(MUTED),
                );
                ui.add_space(4.0);
                if selectable_truncated(
                    ui,
                    self.workspace == Workspace::Project,
                    &self.document.title,
                    28,
                )
                .clicked()
                {
                    self.workspace = Workspace::Project;
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("TABLEAUX").small().strong().color(MUTED));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_sized([58.0, 24.0], egui::Button::new("New"))
                            .on_hover_text("Add tableau")
                            .clicked()
                        {
                            self.add_tableau();
                        }
                        if ui
                            .add_sized([58.0, 24.0], egui::Button::new("Copy"))
                            .on_hover_text("Duplicate tableau")
                            .clicked()
                        {
                            self.duplicate_tableau();
                        }
                    });
                });
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for index in 0..self.document.dataset.len() {
                            let input = if !self.document.dataset[index].name.trim().is_empty() {
                                self.document.dataset[index].name.clone()
                            } else if self.document.dataset[index].input.is_empty() {
                                format!("Tableau {}", index + 1)
                            } else {
                                self.document.dataset[index].input.clone()
                            };
                            if selectable_truncated(
                                ui,
                                self.workspace == Workspace::Tableau
                                    && self.active_tableau == index,
                                &input,
                                28,
                            )
                            .clicked()
                            {
                                self.active_tableau = index;
                                self.workspace = Workspace::Tableau;
                                self.selected_candidate = 0;
                                self.selected_constraint = 0;
                                self.selected_violation = None;
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_sized([54.0, 24.0], egui::Button::new("Up"))
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        self.move_tableau(-1);
                    }
                    if ui
                        .add_sized([54.0, 24.0], egui::Button::new("Down"))
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        self.move_tableau(1);
                    }
                    if ui
                        .add_sized([66.0, 24.0], egui::Button::new("Remove"))
                        .on_hover_text("Remove tableau")
                        .clicked()
                    {
                        self.remove_tableau();
                    }
                });
                ui.add_space(10.0);
                ui.label(RichText::new("TOOLS").small().strong().color(MUTED));
                for (workspace, label) in Workspace::TOOLS {
                    if selectable_truncated(ui, self.workspace == workspace, label, 28).clicked() {
                        self.workspace = workspace;
                    }
                }
            });
    }

    fn inspector(&mut self, context: &egui::Context) {
        if !self.show_inspector {
            return;
        }
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(286.0)
            .width_range(230.0..=420.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .inner_margin(10.0),
            )
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.label(RichText::new("INSPECTOR").small().strong().color(MUTED));
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new("Document")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.label("Title");
                            if ui.text_edit_singleline(&mut self.document.title).changed() {
                                self.dirty = true;
                            }
                            ui.label("Author");
                            if ui.text_edit_singleline(&mut self.document.author).changed() {
                                self.dirty = true;
                            }
                            ui.label("Description");
                            if ui
                                .text_edit_multiline(&mut self.document.description)
                                .changed()
                            {
                                self.dirty = true;
                            }
                            ui.label("Keywords (comma separated)");
                            let mut keywords = self.document.keywords.join(", ");
                            if ui.text_edit_singleline(&mut keywords).changed() {
                                self.document.keywords = keywords
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|item| !item.is_empty())
                                    .map(str::to_owned)
                                    .collect();
                                self.dirty = true;
                            }
                        });
                    match self.workspace {
                        Workspace::Project => self.project_inspector(ui),
                        Workspace::Tableau => self.tableau_inspector(ui),
                        Workspace::SecondOrder => self.second_order_inspector(ui),
                        Workspace::Serial => self.serial_inspector(ui),
                        Workspace::QCalculus => self.q_inspector(ui),
                        Workspace::Plots => self.plot_inspector(ui),
                        Workspace::Diagnostics => self.diagnostic_inspector(ui),
                        Workspace::PhonoScript => self.phont_inspector(ui),
                    }
                });
            });
    }

    fn project_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Presentation and export")
            .default_open(true)
            .show(ui, |ui| {
                self.dirty |= ui
                    .checkbox(
                        &mut self.document.presentation.compact_rows,
                        "Compact tableau rows",
                    )
                    .changed();
                self.dirty |= ui
                    .checkbox(&mut self.document.presentation.show_title, "Show title")
                    .changed();
                self.dirty |= ui
                    .checkbox(&mut self.document.presentation.show_author, "Show author")
                    .changed();
                self.dirty |= ui
                    .checkbox(&mut self.document.presentation.show_legend, "Show legend")
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("PNG scale");
                    self.dirty |= ui
                        .add(
                            egui::DragValue::new(&mut self.document.presentation.export_scale)
                                .range(0.5..=4.0)
                                .speed(0.25)
                                .suffix("×"),
                        )
                        .changed();
                });
                ui.label(
                    RichText::new(
                        "SVG and PDF remain vector-editable. PNG scale controls raster resolution.",
                    )
                    .small()
                    .color(MUTED),
                );
            });
        egui::CollapsingHeader::new("Project operations")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("New tableau").clicked() {
                    self.add_tableau();
                }
                if ui.button("Duplicate current tableau").clicked() {
                    self.duplicate_tableau();
                }
                if ui.button("Apply current constraints to all").clicked() {
                    self.apply_constraints_to_project();
                }
                if ui.button("Export all tableaux…").clicked() {
                    self.export_project_dialog();
                }
            });
    }

    fn tableau_inspector(&mut self, ui: &mut egui::Ui) {
        let table_index = self.active_tableau;
        if self.document.dataset.is_empty() {
            return;
        }
        let candidate_count = self.document.dataset[table_index].candidates.len();
        let constraint_count = self.document.dataset[table_index].constraints.len();
        self.selected_candidate = self
            .selected_candidate
            .min(candidate_count.saturating_sub(1));
        self.selected_constraint = self
            .selected_constraint
            .min(constraint_count.saturating_sub(1));
        egui::CollapsingHeader::new("Tableau")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Name");
                if ui
                    .text_edit_singleline(&mut self.document.dataset[table_index].name)
                    .changed()
                {
                    self.dirty = true;
                }
                ui.label("Input");
                if ui
                    .text_edit_singleline(&mut self.document.dataset[table_index].input)
                    .changed()
                {
                    self.dirty = true;
                }
                ui.label("Tie policy");
                let mut tie_policy = self.document.dataset[table_index].tie_policy_kind();
                if egui::ComboBox::from_id_salt("inspector-tie-policy")
                    .selected_text(tie_policy.label())
                    .show_ui(ui, |ui| {
                        for policy in TiePolicy::ALL {
                            ui.selectable_value(&mut tie_policy, policy, policy.label());
                        }
                    })
                    .response
                    .changed()
                {
                    self.document.dataset[table_index].set_tie_policy(tie_policy);
                    self.dirty = true;
                }
                ui.label("Tableau notes");
                if ui
                    .text_edit_multiline(&mut self.document.dataset[table_index].notes)
                    .changed()
                {
                    self.dirty = true;
                }
            });
        egui::CollapsingHeader::new("Selected candidate")
            .default_open(true)
            .show(ui, |ui| {
                let labels: Vec<String> = self.document.dataset[table_index]
                    .candidates
                    .iter()
                    .map(|candidate| candidate.name.clone())
                    .collect();
                egui::ComboBox::from_id_salt("candidate-selection")
                    .selected_text(
                        labels
                            .get(self.selected_candidate)
                            .map_or("—", String::as_str),
                    )
                    .show_ui(ui, |ui| {
                        for (index, label) in labels.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_candidate, index, label);
                        }
                    });
                if let Some(candidate) = self.document.dataset[table_index]
                    .candidates
                    .get_mut(self.selected_candidate)
                {
                    ui.label("Identity");
                    self.dirty |= ui.text_edit_singleline(&mut candidate.name).changed();
                    ui.label("Candidate form");
                    self.dirty |= ui.text_edit_singleline(&mut candidate.form).changed();
                    ui.horizontal(|ui| {
                        ui.label("Observed");
                        self.dirty |=
                            drag_scalar(ui, &mut candidate.observed_frequency, 0.0..=f64::MAX, 0.1);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Base mass");
                        self.dirty |= drag_scalar(
                            ui,
                            &mut candidate.base_mass,
                            f64::MIN_POSITIVE..=f64::MAX,
                            0.1,
                        );
                    });
                    ui.label("Notes");
                    self.dirty |= ui.text_edit_multiline(&mut candidate.notes).changed();
                }
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        self.add_candidate();
                    }
                    if ui.button("Duplicate").clicked() {
                        self.duplicate_candidate();
                    }
                    if ui.button("Remove").clicked() {
                        self.remove_candidate();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Move up").clicked() {
                        self.move_candidate(-1);
                    }
                    if ui.button("Move down").clicked() {
                        self.move_candidate(1);
                    }
                });
            });
        egui::CollapsingHeader::new("Selected constraint")
            .default_open(true)
            .show(ui, |ui| {
                let labels: Vec<String> = self.document.dataset[table_index]
                    .constraints
                    .iter()
                    .map(|constraint| constraint.name.clone())
                    .collect();
                egui::ComboBox::from_id_salt("constraint-selection")
                    .selected_text(
                        labels
                            .get(self.selected_constraint)
                            .map_or("—", String::as_str),
                    )
                    .show_ui(ui, |ui| {
                        for (index, label) in labels.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_constraint, index, label);
                        }
                    });
                if let Some(constraint) = self.document.dataset[table_index]
                    .constraints
                    .get_mut(self.selected_constraint)
                {
                    ui.label("Name");
                    self.dirty |= ui.text_edit_singleline(&mut constraint.name).changed();
                    ui.horizontal(|ui| {
                        ui.label("Stratum");
                        self.dirty |= ui
                            .add(egui::DragValue::new(&mut constraint.stratum).range(0..=999))
                            .changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Weight");
                        self.dirty |=
                            drag_optional_scalar(ui, &mut constraint.weight, 0.0..=f64::MAX, 0.1);
                    });
                    self.dirty |= ui.checkbox(&mut constraint.enabled, "Enabled").changed();
                    ui.label("Definition").on_hover_text(
                        "Descriptive prose only. The phonologist enters every violation count in the tableau.",
                    );
                    self.dirty |= ui
                        .text_edit_singleline(&mut constraint.definition)
                        .on_hover_text("Constraint definitions are never executed or used to infer marks.")
                        .changed();
                    ui.label("Gaussian prior μ / σ");
                    ui.horizontal(|ui| {
                        self.dirty |=
                            drag_scalar(ui, &mut constraint.prior_mean, f64::MIN..=f64::MAX, 0.1);
                        self.dirty |=
                            drag_scalar(ui, &mut constraint.prior_sigma, 0.000_001..=f64::MAX, 1.0);
                    });
                }
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        self.add_constraint();
                    }
                    if ui.button("Remove").clicked() {
                        self.remove_constraint();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Move left").clicked() {
                        self.move_constraint(-1);
                    }
                    if ui.button("Move right").clicked() {
                        self.move_constraint(1);
                    }
                });
                if self.active_evaluator() == EvaluatorKind::Ot {
                    ui.horizontal(|ui| {
                        if ui.button("Tie left").clicked() {
                            self.tie_constraint_left();
                        }
                        if ui.button("Make strict").clicked() {
                            self.make_constraint_order_strict();
                        }
                    });
                }
            });
    }

    fn second_order_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Typed query")
            .default_open(true)
            .show(ui, |ui| {
                egui::ComboBox::from_id_salt("query-kind")
                    .selected_text(self.document.second_order.query.label())
                    .show_ui(ui, |ui| {
                        for query in QueryKind::ALL {
                            self.dirty |= ui
                                .selectable_value(
                                    &mut self.document.second_order.query,
                                    query,
                                    query.label(),
                                )
                                .changed();
                        }
                    });
                ui.label("Answer sort");
                self.dirty |= ui
                    .text_edit_multiline(&mut self.document.second_order.answer_sort)
                    .changed();
                ui.label("Exact scope");
                self.dirty |= ui
                    .text_edit_multiline(&mut self.document.second_order.scope)
                    .changed();
                ui.label("Transformation");
                self.dirty |= ui
                    .text_edit_multiline(&mut self.document.second_order.transformation)
                    .changed();
                ui.label("Transport");
                self.dirty |= ui
                    .text_edit_multiline(&mut self.document.second_order.transport)
                    .on_hover_text(
                        "identity · rename source=target · fusion a+b=ab; mass-preserving",
                    )
                    .changed();
            });
        egui::CollapsingHeader::new("Comparison contract")
            .default_open(true)
            .show(ui, |ui| {
                ui.label("Response domain");
                for domain in ResponseDomain::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.second_order.response_domain,
                            domain,
                            domain.label(),
                        )
                        .changed();
                }
                ui.separator();
                ui.label("Comparison precision");
                for mode in ComparisonMode::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.second_order.comparison_mode,
                            mode,
                            mode.label(),
                        )
                        .changed();
                }
                match self.document.second_order.comparison_mode {
                    ComparisonMode::Exact => {
                        ui.label(
                            RichText::new(
                                "Discrete equality or certified exact MaxEnt exponential-polynomial identity.",
                            )
                            .small()
                            .color(MUTED),
                        );
                    }
                    ComparisonMode::Approximate => {
                        ui.horizontal(|ui| {
                            ui.label("Tolerance");
                            self.dirty |= drag_scalar(
                                ui,
                                &mut self.document.second_order.tolerance,
                                0.0..=1.0,
                                1.0e-6,
                            );
                        });
                    }
                    ComparisonMode::Grid => {
                        ui.horizontal(|ui| {
                            ui.label("Grid step");
                            self.dirty |= drag_scalar(
                                ui,
                                &mut self.document.second_order.grid_step,
                                1.0e-12..=1.0,
                                1.0e-4,
                            );
                        });
                    }
                }
                ui.separator();
                ui.label("MaxEnt normalizer");
                for policy in NormalizerPolicy::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.second_order.normalizer_policy,
                            policy,
                            policy.label(),
                        )
                        .changed();
                }
            });
        egui::CollapsingHeader::new("Layer and consumer")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Source scientific layer");
                self.dirty |= ui
                    .text_edit_singleline(&mut self.document.second_order.source_layer)
                    .changed();
                ui.label("Target scientific layer");
                self.dirty |= ui
                    .text_edit_singleline(&mut self.document.second_order.target_layer)
                    .changed();
                ui.label("Layer transport");
                self.dirty |= ui
                    .text_edit_singleline(&mut self.document.second_order.layer_transport)
                    .changed();
                ui.separator();
                for mode in ConsumerMode::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.second_order.consumer_mode,
                            mode,
                            mode.label(),
                        )
                        .changed();
                }
                if self.document.second_order.consumer_mode == ConsumerMode::LaterConsumer {
                    ui.label("Consumer");
                    self.dirty |= ui
                        .text_edit_singleline(&mut self.document.second_order.consumer)
                        .on_hover_text(
                            "identity · winner-set · support-cardinality · terminal-output",
                        )
                        .changed();
                }
            });
        egui::CollapsingHeader::new("Display")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Display geometry");
                for layout in crate::model::SecondOrderLayout::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.second_order.layout,
                            layout,
                            layout.label(),
                        )
                        .changed();
                }
            });
    }

    fn serial_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Serial contract")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.edit_target, false, "Source");
                    ui.selectable_value(&mut self.edit_target, true, "Target");
                });
                let settings = if self.edit_target {
                    &mut self.document.target_serial
                } else {
                    &mut self.document.serial
                };
                ui.label("Initial form");
                self.dirty |= ui
                    .text_edit_singleline(&mut settings.start)
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Maximum steps");
                    self.dirty |= ui
                        .add(
                            egui::DragValue::new(&mut settings.maximum_steps).range(1..=100_000),
                        )
                        .changed();
                });
                ui.label(
                    RichText::new(
                        "Each local GEN set must contain identity. One ranking is reused at every pass.",
                    )
                    .small()
                    .color(MUTED),
                );
            });
    }

    fn q_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Representation audit")
            .default_open(true)
            .show(ui, |ui| {
                let constraints: Vec<String> = self
                    .active()
                    .constraints
                    .iter()
                    .map(|constraint| constraint.name.clone())
                    .collect();
                let selected = constraints
                    .get(self.document.clone_constraint)
                    .map_or("—", String::as_str);
                egui::ComboBox::from_id_salt("clone-constraint")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (index, constraint) in constraints.iter().enumerate() {
                            self.dirty |= ui
                                .selectable_value(
                                    &mut self.document.clone_constraint,
                                    index,
                                    constraint,
                                )
                                .changed();
                        }
                    });
                ui.label(
                    RichText::new(
                        "The audit compares exact ranking support and exact combinatorial shares after a constraint clone.",
                    )
                    .small()
                    .color(MUTED),
                );
            });
    }

    fn plot_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Plot")
            .default_open(true)
            .show(ui, |ui| {
                for kind in PlotKind::ALL {
                    self.dirty |= ui
                        .radio_value(
                            &mut self.document.plot,
                            kind,
                            kind.label_for(self.document.evaluator),
                        )
                        .changed();
                }
                ui.separator();
                ui.label("Export format");
                ui.horizontal(|ui| {
                    for format in ExportFormat::ALL {
                        ui.radio_value(&mut self.export_format, format, format.label());
                    }
                });
                if ui.button("Export plot…").clicked() {
                    self.export_dialog("plot");
                }
            });
    }

    fn diagnostic_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Automated analysis")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("Infer OT ranking").clicked() {
                    self.infer_ranking();
                }
                if ui.button("Learn MaxEnt weights").clicked() {
                    self.learn_weights();
                }
                if ui.button("Run structural diagnostics").clicked() {
                    self.run_diagnostics();
                }
                if ui.button("Compute factorial typology").clicked() {
                    self.compute_typology();
                }
            });
    }

    fn phont_inspector(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("PhonoScript execution")
            .default_open(true)
            .show(ui, |ui| {
                if ui
                    .button("Run transaction")
                    .on_hover_text(
                        "Commit project changes only after parsing, execution, normalization, and validation succeed",
                    )
                    .clicked()
                {
                    self.run_phont();
                }
                if ui.button("Save script").clicked() {
                    self.save_phont();
                }
                if ui.button("Language reference").clicked() {
                    self.phont_output.extend(phonoscript_reference());
                }
            });
    }

    fn console_panel(&mut self, context: &egui::Context) {
        if !self.show_console {
            return;
        }
        egui::TopBottomPanel::bottom("console")
            .resizable(true)
            .default_height(150.0)
            .height_range(92.0..=360.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(32, 38, 44))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(76, 85, 94)))
                    .inner_margin(egui::Margin::symmetric(10, 7)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("COMMAND")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(195, 204, 211)),
                    );
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.command)
                            .id(egui::Id::new("command-line"))
                            .desired_width(f32::INFINITY)
                            .hint_text("Type help or a command")
                            .text_color(Color32::WHITE),
                    );
                    if (response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                        || ui.button("Run").clicked()
                    {
                        self.run_command();
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in self.console.iter().rev().take(80).rev() {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(11.5)
                                    .color(Color32::from_rgb(220, 225, 229)),
                            );
                        }
                    });
            });
    }

    fn status_bar(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .inner_margin(egui::Margin::symmetric(9, 4)),
            )
            .show(context, |ui| {
                let candidates = self.active().candidates.len();
                let constraints = self.active().constraints.len();
                let status = self.status.clone();
                let reserved = if ui.available_width() < 720.0 {
                    205.0
                } else {
                    280.0
                };
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [(ui.available_width() - reserved).max(72.0), 18.0],
                        egui::Label::new(RichText::new(&status).small().color(MUTED)).truncate(),
                    )
                    .on_hover_text(&status);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("ConvalGEN {}", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(MUTED),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format!(
                                "{} · {}",
                                counted(candidates, "candidate", "candidates"),
                                counted(constraints, "constraint", "constraints")
                            ))
                            .small()
                            .color(MUTED),
                        );
                    });
                });
            });
    }

    fn workspace(&mut self, context: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(SURFACE).inner_margin(14.0))
            .show(context, |ui| match self.workspace {
                Workspace::Project => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-project-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.project_workspace(ui));
                }
                Workspace::Tableau => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-tableau-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.tableau_workspace(ui));
                }
                Workspace::Serial => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-serial-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.serial_workspace(ui));
                }
                Workspace::SecondOrder => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-second-order-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.second_order_workspace(ui));
                }
                Workspace::Diagnostics => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-diagnostics-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.diagnostics_workspace(ui));
                }
                Workspace::QCalculus => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-q-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.q_workspace(ui));
                }
                Workspace::Plots => self.plots_workspace(ui),
                Workspace::PhonoScript => {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-phonoscript-vertical")
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.phont_workspace(ui));
                }
            });
    }

    fn heading(ui: &mut egui::Ui, title: &str) {
        ui.heading(title);
        ui.add_space(5.0);
    }

    fn project_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Project overview");
        let tableau_count = self.document.dataset.len();
        let candidate_count: usize = self
            .document
            .dataset
            .iter()
            .map(|tableau| tableau.candidates.len())
            .sum();
        let constraint_count = self
            .document
            .dataset
            .iter()
            .map(|tableau| tableau.constraints.len())
            .max()
            .unwrap_or(0);
        theme::section().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong(counted(tableau_count, "tableau", "tableaux"));
                ui.separator();
                ui.label(counted(
                    candidate_count,
                    "candidate record",
                    "candidate records",
                ));
                ui.separator();
                ui.label(format!(
                    "up to {} per tableau",
                    counted(constraint_count, "constraint", "constraints")
                ));
                ui.separator();
                ui.label(self.document.evaluator.label());
            });
            if !self.document.description.trim().is_empty() {
                ui.separator();
                ui.label(&self.document.description);
            }
        });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if project_button(ui, "New tableau", "Add a tableau to this project") {
                self.add_tableau();
            }
            if project_button(ui, "Duplicate selected", "Duplicate the selected tableau") {
                self.duplicate_tableau();
            }
            if project_button(
                ui,
                "Apply constraints to all",
                "Replace every project tableau's constraint set with the active set",
            ) {
                self.apply_constraints_to_project();
            }
            if project_button(ui, "Export project…", "Export every project tableau") {
                self.export_project_dialog();
            }
        });
        ui.add_space(10.0);
        egui::ScrollArea::both()
            .id_salt("project-tableaux-scroll")
            .max_height((ui.clip_rect().height() * 0.62).max(220.0))
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                egui::Grid::new("project-tableaux")
                    .striped(true)
                    .min_col_width(96.0)
                    .show(ui, |ui| {
                        ui.strong("Tableau");
                        ui.strong("Input");
                        ui.strong("Candidates");
                        ui.strong("Constraints");
                        ui.strong("Evaluator");
                        ui.strong("Calculated result");
                        ui.strong("Regression expectation");
                        ui.end_row();
                        for index in 0..self.document.dataset.len() {
                            let tableau = &self.document.dataset[index];
                            let kind = tableau.evaluator_or(self.document.evaluator);
                            let temperature = tableau.temperature_or(&self.document.temperature);
                            let evaluation =
                                PhonologicalEngine::new().evaluate(tableau, kind, temperature);
                            let (winners, tie_unresolved, mut actual, refusal) = match &evaluation {
                                Ok(evaluation) => (
                                    evaluation
                                        .winner_indices
                                        .iter()
                                        .map(|winner| tableau.candidates[*winner].name.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    evaluation.tie_unresolved,
                                    evaluation
                                        .winner_indices
                                        .iter()
                                        .map(|winner| tableau.candidates[*winner].name.clone())
                                        .collect::<Vec<_>>(),
                                    None,
                                ),
                                Err(problem) => (
                                    String::new(),
                                    false,
                                    Vec::new(),
                                    Some(format!("{} · {}", problem.code, problem.message)),
                                ),
                            };
                            let label = if tableau.name.trim().is_empty() {
                                format!("Tableau {}", index + 1)
                            } else {
                                tableau.name.clone()
                            };
                            if ui
                                .selectable_label(self.active_tableau == index, label)
                                .clicked()
                            {
                                self.active_tableau = index;
                                self.workspace = Workspace::Tableau;
                            }
                            ui.label(&tableau.input);
                            ui.label(tableau.candidates.len().to_string());
                            ui.label(tableau.constraints.len().to_string());
                            ui.label(kind.short_label());
                            ui.label(if let Some(problem) = refusal {
                                format!("REFUSED · {problem}")
                            } else if kind == EvaluatorKind::MaxEnt {
                                format!("modal: {winners}")
                            } else if tie_unresolved {
                                "unresolved required-unique tie".to_owned()
                            } else {
                                winners.clone()
                            });
                            let mut expected = tableau.expected_winners.clone();
                            expected.sort();
                            actual.sort();
                            if expected.is_empty() {
                                ui.label(RichText::new("—").color(MUTED));
                            } else if expected == actual {
                                ui.label(RichText::new("Verified").color(FOCUS));
                            } else {
                                ui.label(RichText::new("Mismatch").color(NEGATIVE));
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    fn tableau_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Tableau editor");
        ui.horizontal_wrapped(|ui| {
            ui.label("Filter rows");
            ui.add(
                egui::TextEdit::singleline(&mut self.row_filter)
                    .desired_width(220.0)
                    .hint_text("candidate label or form"),
            );
            ui.separator();
            ui.label("Tie handling");
            let mut tie_policy = self.active().tie_policy_kind();
            if egui::ComboBox::from_id_salt("workspace-tie-policy")
                .width(180.0)
                .selected_text(tie_policy.label())
                .show_ui(ui, |ui| {
                    for policy in TiePolicy::ALL {
                        ui.selectable_value(&mut tie_policy, policy, policy.label());
                    }
                })
                .response
                .changed()
            {
                self.set_tie_policy(tie_policy);
            }
        });
        ui.add_space(5.0);
        theme::section().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("CANDIDATE").small().strong().color(MUTED));
                if action_button(ui, "Add", "Add candidate (Command+Enter)") {
                    self.add_candidate();
                }
                if action_button(ui, "Duplicate", "Duplicate selected candidate (Command+D)") {
                    self.duplicate_candidate();
                }
                if action_button(ui, "Move up", "Move selected candidate up (Option+Up)") {
                    self.move_candidate(-1);
                }
                if action_button(
                    ui,
                    "Move down",
                    "Move selected candidate down (Option+Down)",
                ) {
                    self.move_candidate(1);
                }
                if action_button(ui, "Remove", "Remove selected candidate") {
                    self.remove_candidate();
                }
            });
            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("CONSTRAINT").small().strong().color(MUTED));
                if action_button(ui, "Add", "Add constraint (Shift+Command+Enter)") {
                    self.add_constraint();
                }
                if action_button(ui, "Move left", "Move and re-rank left (Option+Left)") {
                    self.move_constraint(-1);
                }
                if action_button(ui, "Move right", "Move and re-rank right (Option+Right)") {
                    self.move_constraint(1);
                }
                if self.document.evaluator == EvaluatorKind::Ot
                    && action_button(
                        ui,
                        "Tie left",
                        "Place the selected constraint in the preceding stratum",
                    )
                {
                    self.tie_constraint_left();
                }
                if self.document.evaluator == EvaluatorKind::Ot
                    && action_button(
                        ui,
                        "Make strict",
                        "Remove all stratum ties and use column order",
                    )
                {
                    self.make_constraint_order_strict();
                }
                if action_button(ui, "Remove", "Remove selected constraint") {
                    self.remove_constraint();
                }
            });
        });
        ui.add_space(6.0);
        let kind = self.active_evaluator();
        let temperature = self.active_temperature();
        let compact = self.document.presentation.compact_rows;
        let filter = self.row_filter.clone();
        let mut selected_candidate = self.selected_candidate;
        let mut selected_constraint = self.selected_constraint;
        let mut selected_violation = self.selected_violation;
        let active_index = self
            .active_tableau
            .min(self.document.dataset.len().saturating_sub(1));
        let (elapsed, changed) = Self::tableau_editor(
            ui,
            &mut self.document.dataset[active_index],
            kind,
            temperature,
            compact,
            &filter,
            &mut selected_candidate,
            &mut selected_constraint,
            &mut selected_violation,
        );
        self.selected_candidate = selected_candidate;
        self.selected_constraint = selected_constraint;
        self.selected_violation = selected_violation;
        self.last_evaluation = elapsed;
        self.dirty |= changed;
        ui.add_space(8.0);
        let evaluation = PhonologicalEngine::new().evaluate(self.active(), kind, temperature);
        theme::section().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("DERIVED RESULT")
                        .small()
                        .strong()
                        .color(MUTED),
                );
                ui.label(RichText::new("·").color(MUTED));
                match &evaluation {
                    Err(problem) => {
                        ui.label(
                            RichText::new(format!("REFUSED · {problem}"))
                                .strong()
                                .color(NEGATIVE),
                        );
                    }
                    Ok(evaluation) if kind == EvaluatorKind::MaxEnt => {
                        let probabilities = evaluation
                            .rows
                            .iter()
                            .map(|row| {
                                format!(
                                    "{}  p={:.6}",
                                    self.active().candidates[row.candidate].name,
                                    row.probability.unwrap_or(0.0)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let limit = if ui.available_width() < 520.0 {
                            56
                        } else {
                            112
                        };
                        ui.label(truncate(&probabilities, limit))
                            .on_hover_text(probabilities);
                    }
                    Ok(evaluation) => {
                        let winners = evaluation
                            .winner_indices
                            .iter()
                            .map(|index| self.active().candidates[*index].name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        if evaluation.tie_unresolved {
                            ui.label(
                                RichText::new("unresolved tie: a unique winner is required")
                                    .strong()
                                    .color(NEGATIVE),
                            );
                        } else {
                            ui.label(RichText::new(format!("winner set: {winners}")).strong());
                        }
                        ui.label(RichText::new("·").color(MUTED));
                        ui.label(format!(
                            "tie policy: {}",
                            self.active().tie_policy_kind().label()
                        ));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(speed(elapsed)).small().color(MUTED));
                });
            });
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn tableau_editor(
        ui: &mut egui::Ui,
        tableau: &mut Tableau,
        kind: EvaluatorKind,
        temperature: f64,
        compact: bool,
        filter: &str,
        selected_candidate: &mut usize,
        selected_constraint: &mut usize,
        selected_violation: &mut Option<ViolationCell>,
    ) -> (Duration, bool) {
        tableau.normalize();
        let started = Instant::now();
        let evaluation = PhonologicalEngine::new().evaluate(tableau, kind, temperature);
        let elapsed = started.elapsed();
        let row_height = if compact { 29.0 } else { 38.0 };
        let filter = filter.to_lowercase();
        let visible: Vec<usize> = tableau
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                filter.is_empty()
                    || candidate.name.to_lowercase().contains(&filter)
                    || candidate.form.to_lowercase().contains(&filter)
            })
            .map(|(index, _)| index)
            .collect();
        let constraint_count = tableau.constraints.len();
        let mut changed = false;
        let metric_columns = if kind == EvaluatorKind::MaxEnt { 3 } else { 1 };
        let nominal_width =
            155.0 + 105.0 + constraint_count as f32 * 80.0 + metric_columns as f32 * 76.0;
        let spare_width = (ui.available_width() - nominal_width).clamp(0.0, 320.0);
        let candidate_width = 155.0 + spare_width * 0.42;
        let form_width = 105.0 + spare_width * 0.58;
        let body_height = tableau_body_height(row_height, visible.len(), ui.clip_rect().height());
        egui::ScrollArea::horizontal()
            .id_salt(("tableau-horizontal", tableau.id.clone()))
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                let mut builder = TableBuilder::new(ui)
                    .id_salt(("tableau-body", tableau.id.clone()))
                    .striped(true)
                    .resizable(false)
                    .sense(Sense::click())
                    .min_scrolled_height(body_height)
                    .max_scroll_height(body_height)
                    .column(Column::initial(candidate_width).at_least(110.0))
                    .column(Column::initial(form_width).at_least(85.0));
                for _ in 0..constraint_count {
                    builder = builder.column(Column::initial(80.0).at_least(64.0));
                }
                if kind == EvaluatorKind::MaxEnt {
                    builder = builder
                        .column(Column::initial(68.0).at_least(62.0))
                        .column(Column::initial(68.0).at_least(62.0));
                }
                builder = builder.column(Column::initial(85.0).at_least(75.0));
                builder
                    .header(50.0, |mut header| {
                        header.col(|ui| {
                            ui.label(RichText::new("Input").small().strong().color(MUTED));
                            changed |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut tableau.input)
                                        .frame(false)
                                        .hint_text("/input/"),
                                )
                                .changed();
                        });
                        header.col(|ui| {
                            ui.label(RichText::new("Form").strong());
                        });
                        for index in 0..constraint_count {
                            header.col(|ui| {
                                let constraint = &mut tableau.constraints[index];
                                let name = ui.add(
                                    egui::TextEdit::singleline(&mut constraint.name)
                                        .frame(false)
                                        .desired_width(f32::INFINITY),
                                );
                                changed |= name.changed();
                                if name.clicked() || name.gained_focus() {
                                    *selected_constraint = index;
                                }
                                ui.horizontal(|ui| match kind {
                                    EvaluatorKind::Ot => {
                                        ui.label(RichText::new("rank").small().color(MUTED));
                                        let mut rank = constraint.stratum + 1;
                                        if ui
                                            .add(
                                                egui::DragValue::new(&mut rank)
                                                    .range(1..=usize::MAX),
                                            )
                                            .changed()
                                        {
                                            constraint.stratum = rank.saturating_sub(1);
                                            changed = true;
                                        }
                                    }
                                    EvaluatorKind::HarmonicGrammar | EvaluatorKind::MaxEnt => {
                                        ui.label(RichText::new("w").small().color(MUTED));
                                        changed |= drag_optional_scalar(
                                            ui,
                                            &mut constraint.weight,
                                            0.0..=f64::MAX,
                                            0.05,
                                        );
                                    }
                                });
                            });
                        }
                        if kind == EvaluatorKind::MaxEnt {
                            header.col(|ui| {
                                ui.label(RichText::new("Observed\ncount").strong());
                            });
                            header.col(|ui| {
                                ui.label(RichText::new("Prior mass\nρ").strong());
                            });
                        }
                        header.col(|ui| {
                            ui.label(
                                RichText::new(match kind {
                                    EvaluatorKind::Ot => "Rank tier",
                                    EvaluatorKind::HarmonicGrammar => "Cost",
                                    EvaluatorKind::MaxEnt => "Probability",
                                })
                                .strong(),
                            );
                        });
                    })
                    .body(|body| {
                        body.rows(row_height, visible.len(), |mut row| {
                            let candidate_index = visible[row.index()];
                            let row_selected = *selected_candidate == candidate_index;
                            let result = evaluation
                                .as_ref()
                                .ok()
                                .and_then(|evaluation| evaluation.rows.get(candidate_index));
                            row.set_selected(row_selected);
                            let candidate = &mut tableau.candidates[candidate_index];
                            row.col(|ui| {
                                ui.horizontal(|ui| {
                                    let winner = result.is_some_and(|result| result.winner);
                                    let winner_color = if candidate_index == *selected_candidate {
                                        Color32::WHITE
                                    } else {
                                        FOCUS
                                    };
                                    ui.label(if winner {
                                        RichText::new("W").strong().color(winner_color)
                                    } else {
                                        RichText::new("")
                                    })
                                    .on_hover_text(
                                        if winner {
                                            "Winning candidate"
                                        } else {
                                            "Candidate"
                                        },
                                    );
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(&mut candidate.name)
                                                .frame(false),
                                        )
                                        .changed();
                                });
                            });
                            row.col(|ui| {
                                changed |= ui
                                    .add(
                                        egui::TextEdit::singleline(&mut candidate.form)
                                            .frame(false),
                                    )
                                    .changed();
                            });
                            for constraint_index in 0..constraint_count {
                                row.col(|ui| {
                                    if row_selected {
                                        ui.visuals_mut().override_text_color = Some(INK);
                                    }
                                    let cell = ViolationCell {
                                        candidate: candidate_index,
                                        constraint: constraint_index,
                                    };
                                    let response = violation_editor(
                                        ui,
                                        &mut candidate.violations[constraint_index],
                                    );
                                    changed |= response.changed();
                                    if response.clicked() || response.gained_focus() {
                                        *selected_violation = Some(cell);
                                        *selected_candidate = candidate_index;
                                        *selected_constraint = constraint_index;
                                    }
                                    if *selected_violation == Some(cell) {
                                        ui.painter().rect_stroke(
                                            response.rect.expand(2.0),
                                            2.0,
                                            Stroke::new(1.5_f32, FOCUS),
                                            StrokeKind::Inside,
                                        );
                                    }
                                    if result.is_some_and(|result| {
                                        result.fatal_constraint == Some(constraint_index)
                                    }) {
                                        ui.label(RichText::new("!").strong().color(INK));
                                    }
                                });
                            }
                            if kind == EvaluatorKind::MaxEnt {
                                row.col(|ui| {
                                    if row_selected {
                                        ui.visuals_mut().override_text_color = Some(INK);
                                    }
                                    changed |= drag_scalar(
                                        ui,
                                        &mut candidate.observed_frequency,
                                        0.0..=f64::MAX,
                                        0.25,
                                    );
                                });
                                row.col(|ui| {
                                    if row_selected {
                                        ui.visuals_mut().override_text_color = Some(INK);
                                    }
                                    changed |= drag_scalar(
                                        ui,
                                        &mut candidate.base_mass,
                                        f64::MIN_POSITIVE..=f64::MAX,
                                        0.05,
                                    );
                                });
                            }
                            row.col(|ui| {
                                let rank_tier = evaluation.as_ref().ok().and_then(|evaluation| {
                                    evaluation
                                        .ordered_strata
                                        .iter()
                                        .position(|tier| tier.contains(&candidate_index))
                                        .map(|index| index + 1)
                                });
                                let value = match (kind, result) {
                                    (EvaluatorKind::Ot, Some(_)) => {
                                        rank_tier.unwrap_or_default().to_string()
                                    }
                                    (EvaluatorKind::HarmonicGrammar, Some(result)) => result
                                        .exact_harmony
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| format!("~{:.4}", result.harmony)),
                                    (EvaluatorKind::MaxEnt, Some(result)) => {
                                        format!("{:.6}", result.probability.unwrap_or(0.0))
                                    }
                                    (_, None) => "—".to_owned(),
                                };
                                ui.label(RichText::new(value).monospace().strong());
                            });
                            if row.response().clicked() {
                                *selected_candidate = candidate_index;
                            }
                        });
                    });
            });
        (elapsed, changed)
    }

    fn serial_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Serial derivation");
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.edit_target, false, "Source derivation");
            ui.selectable_value(&mut self.edit_target, true, "Target derivation");
        });
        let tableau = if self.edit_target {
            self.document.target.clone()
        } else {
            self.document.source.clone()
        };
        let constraints = tableau.constraints.clone();
        let width = constraints.len();
        let mut remove = None;
        let mut changed = false;
        let settings = if self.edit_target {
            &mut self.document.target_serial
        } else {
            &mut self.document.serial
        };
        let serial_max_height = (ui.clip_rect().height() * 0.52).max(180.0);
        theme::section().show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("serial-moves-scroll")
                .max_height(serial_max_height)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    egui::Grid::new("serial-moves")
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("From");
                            ui.strong("To");
                            ui.strong("Operation");
                            for constraint in &constraints {
                                ui.strong(&constraint.name);
                            }
                            ui.strong("Action");
                            ui.end_row();
                            for (index, item) in settings.moves.iter_mut().enumerate() {
                                changed |= ui.text_edit_singleline(&mut item.from).changed();
                                changed |= ui.text_edit_singleline(&mut item.to).changed();
                                changed |= ui.text_edit_singleline(&mut item.operation).changed();
                                for constraint_index in 0..width {
                                    if let Some(mark) =
                                        item.violations.get_mut(constraint_index)
                                    {
                                        changed |= violation_editor(ui, mark).changed();
                                    } else {
                                        ui.label(RichText::new("—").color(CAUTION))
                                            .on_hover_text("Missing analyst-supplied violation count");
                                    }
                                }
                                ui.vertical(|ui| {
                                    if item.violations.len() != width
                                        && ui
                                            .small_button("Align ledger")
                                            .on_hover_text(
                                                "Resize this row to the constraint register; new cells remain unset until the phonologist enters them",
                                            )
                                            .clicked()
                                    {
                                        item.violations.resize(width, UNSET_VIOLATION);
                                        changed = true;
                                    }
                                    if ui.small_button("Remove").clicked() {
                                        remove = Some(index);
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
            if ui
                .add_sized([142.0, 28.0], egui::Button::new("Add local candidate"))
                .clicked()
            {
                settings.moves.push(SerialMove {
                    from: settings.start.clone(),
                    to: settings.start.clone(),
                    operation: "identity".to_owned(),
                    violations: vec![UNSET_VIOLATION; width],
                });
                changed = true;
            }
        });
        if let Some(index) = remove {
            settings.moves.remove(index);
            changed = true;
        }
        let settings = settings.clone();
        self.dirty |= changed;
        let started = Instant::now();
        let result = PhonologicalEngine::new().serial(
            &tableau,
            &settings,
            tableau.evaluator_or(self.document.evaluator),
            tableau.temperature_or(&self.document.temperature),
        );
        self.last_evaluation = started.elapsed();
        ui.add_space(9.0);
        theme::section().show(ui, |ui| {
            ui.label(RichText::new("DERIVED PATH").small().strong().color(MUTED));
            match result {
                Ok(result) => {
                    ui.label(
                        RichText::new(result.path.join("  ->  "))
                            .size(18.0)
                            .strong(),
                    );
                    if !result.operations.is_empty() {
                        ui.label(format!("operations: {}", result.operations.join(" · ")));
                    }
                    ui.label(
                        RichText::new(&result.stopped)
                            .color(if result.formed { INK } else { NEGATIVE })
                            .strong(),
                    );
                }
                Err(problem) => {
                    ui.label(RichText::new(format!("REFUSED · {problem}")).color(NEGATIVE));
                }
            }
        });
    }

    fn second_order_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Second-Order Tableau");
        let started = Instant::now();
        let result = PhonologicalEngine::new().compare(&self.document);
        self.last_evaluation = started.elapsed();
        theme::section().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Q · {}", self.document.second_order.query.label()))
                        .strong(),
                );
                ui.separator();
                ui.label(format!(
                    "answer sort: {}",
                    self.document.second_order.answer_sort
                ));
                ui.separator();
                ui.label(RichText::new(result.status.label()).strong().color(
                    match result.status {
                        ComparisonStatus::Preserved => FOCUS,
                        ComparisonStatus::Discrepant => NEGATIVE,
                        ComparisonStatus::NotEvaluated => CAUTION,
                    },
                ));
            });
            ui.label(format!(
                "transformation: {}",
                self.document.second_order.transformation
            ));
            ui.label(format!(
                "transport: {}",
                self.document.second_order.transport
            ));
            ui.label(format!("exact scope: {}", self.document.second_order.scope));
            ui.label(format!(
                "{} · {} · {}",
                self.document.second_order.response_domain.label(),
                self.document.second_order.comparison_mode.label(),
                self.document.second_order.normalizer_policy.label()
            ));
            if let (Some(source), Some(target)) =
                (&result.source_normalizer, &result.target_normalizer)
            {
                ui.label(format!("source {source} · target {target}"));
            }
            ui.separator();
            ui.label(format!(
                "source answer: {}",
                format_answer(&result.source_answer)
            ));
            ui.label(format!(
                "transported source answer: {}",
                format_answer(&result.transported_source_answer)
            ));
            ui.label(format!(
                "independently calculated target answer: {}",
                format_answer(&result.target_answer)
            ));
            if let Some(refusal) = &result.refusal {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {}",
                        refusal.code,
                        refusal.stage.label(),
                        refusal.coordinate
                    ))
                    .strong()
                    .color(NEGATIVE),
                );
                ui.label(&refusal.message);
                ui.label(RichText::new(&refusal.remedy).small().color(MUTED));
            }
            if !result.discrepancies.is_empty() {
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "COMPLETE DISCREPANCY RECORD · {}",
                        counted(result.discrepancies.len(), "coordinate", "coordinates")
                    ))
                    .small()
                    .strong()
                    .color(NEGATIVE),
                );
                egui::ScrollArea::horizontal()
                    .id_salt("second-order-discrepancy-scroll")
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .show(ui, |ui| {
                        egui::Grid::new("second-order-discrepancies")
                            .striped(true)
                            .show(ui, |ui| {
                                ui.strong("Coordinate");
                                ui.strong("Source");
                                ui.strong("Target");
                                ui.strong("Reason");
                                ui.end_row();
                                for discrepancy in &result.discrepancies {
                                    ui.label(&discrepancy.coordinate);
                                    ui.label(&discrepancy.source);
                                    ui.label(&discrepancy.target);
                                    ui.label(&discrepancy.difference);
                                    ui.end_row();
                                }
                            });
                    });
            }
            if let Some(certificate) = &result.certificate {
                ui.separator();
                ui.label(
                    RichText::new("PRESERVATION CERTIFICATE")
                        .small()
                        .strong()
                        .color(FOCUS),
                );
                ui.label(&certificate.statement);
                for evidence in &certificate.evidence {
                    ui.label(RichText::new(format!("• {evidence}")).small().color(MUTED));
                }
            }
        });
        ui.add_space(7.0);
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.edit_target, false, "Source analysis");
            ui.selectable_value(&mut self.edit_target, true, "Target analysis");
            ui.separator();
            ui.label(format!(
                "display geometry: {}",
                self.document.second_order.layout.label()
            ));
            if ui.button("Export…").clicked() {
                self.export_dialog("second-order");
            }
        });
        let kind = self.document.evaluator;
        let temperature = self
            .document
            .temperature
            .to_f64_center()
            .unwrap_or(f64::NAN);
        let compact = self.document.presentation.compact_rows;
        let tableau = if self.edit_target {
            &mut self.document.target
        } else {
            &mut self.document.source
        };
        let (elapsed, changed) = Self::tableau_editor(
            ui,
            tableau,
            kind,
            temperature,
            compact,
            "",
            &mut self.selected_candidate,
            &mut self.selected_constraint,
            &mut self.selected_violation,
        );
        self.last_evaluation += elapsed;
        self.dirty |= changed;
    }

    fn diagnostics_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Ranking and diagnostics");
        ui.horizontal_wrapped(|ui| {
            if diagnostic_button(ui, "Infer OT ranking") {
                self.infer_ranking();
            }
            if diagnostic_button(ui, "Learn MaxEnt weights") {
                self.learn_weights();
            }
            if diagnostic_button(ui, "Structural diagnostics") {
                self.run_diagnostics();
            }
            if diagnostic_button(ui, "Factorial typology") {
                self.compute_typology();
            }
        });
        ui.add_space(8.0);
        theme::section().show(ui, |ui| {
            if self.diagnostics.is_empty() {
                ui.label(
                    RichText::new("Choose an analysis action. Results appear here and in the command console.")
                        .color(MUTED),
                );
            } else {
                for line in &self.diagnostics {
                    ui.label(RichText::new(line).monospace());
                }
            }
        });
    }

    fn q_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Q-Calculus representation audit");
        let started = Instant::now();
        match PhonologicalEngine::new().q_clone_audit(
            self.active(),
            self.document.clone_constraint,
            &self.document.a_priori_rankings,
            self.active_evaluator(),
            self.active_temperature(),
        ) {
            Ok(result) => {
                self.last_evaluation = started.elapsed();
                self.q_result(ui, &result);
            }
            Err(error) => {
                theme::section().show(ui, |ui| {
                    ui.label(RichText::new(format!("REFUSED · {error}")).color(NEGATIVE));
                });
            }
        }
    }

    fn q_result(&self, ui: &mut egui::Ui, result: &CloneAuditResult) {
        theme::section().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("SUPPORT").small().strong().color(MUTED));
                ui.label(if result.support_conservative {
                    "conservative"
                } else {
                    "changed"
                });
                ui.separator();
                ui.label(RichText::new("RANKING SHARES").small().strong().color(MUTED));
                ui.label(if result.shares_conservative {
                    "conservative"
                } else {
                    "changed"
                });
                ui.separator();
                ui.label(format!(
                    "{} before; {} after clone",
                    counted(
                        result.before.total_rankings.clone(),
                        "compatible ranking",
                        "compatible rankings"
                    ),
                    counted(
                        result.after.total_rankings.clone(),
                        "compatible ranking",
                        "compatible rankings"
                    )
                ));
            });
            ui.separator();
            egui::ScrollArea::horizontal()
                .id_salt("q-shifts-scroll")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    egui::Grid::new("q-shifts").striped(true).show(ui, |ui| {
                        ui.strong("Answer");
                        ui.strong("Before");
                        ui.strong("After clone");
                        ui.end_row();
                        for shift in &result.shifts {
                            ui.label(shift.answer.join("; "));
                            ui.label(format_share(&shift.before));
                            ui.label(format_share(&shift.after));
                            ui.end_row();
                        }
                    });
                });
            ui.separator();
            ui.label(RichText::new("EXACT CLAIM BOUNDARY").small().strong().color(CAUTION));
            ui.label("Possible-output support is invariant under the declared clone exactly when reported above.");
            ui.label("Uniform ranking fractions are combinatorial shares, not token probabilities.");
            ui.label(
                RichText::new("Token probability is not formed without a declared measure and response law.")
                    .strong(),
            );
        });
    }

    fn plots_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "Plots");
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("plot-kind-main")
                .selected_text(self.document.plot.label_for(self.document.evaluator))
                .show_ui(ui, |ui| {
                    for kind in PlotKind::ALL {
                        ui.selectable_value(
                            &mut self.document.plot,
                            kind,
                            kind.label_for(self.document.evaluator),
                        );
                    }
                });
            if ui.button("Export…").clicked() {
                self.export_dialog("plot");
            }
        });
        ui.add_space(8.0);
        theme::section().show(ui, |ui| {
            let available = ui.available_size().max(Vec2::new(220.0, 240.0));
            let (rect, _) = ui.allocate_exact_size(available, Sense::hover());
            self.paint_plot(ui, rect);
        });
    }

    fn phont_workspace(&mut self, ui: &mut egui::Ui) {
        Self::heading(ui, "PhonoScript script");
        ui.horizontal_wrapped(|ui| {
            if workspace_action_button(ui, "New", "Start a new PhonoScript script") {
                self.new_phont_script();
            }
            if workspace_action_button(ui, "Open…", "Open a PhonoScript script") {
                self.open_phont_dialog();
            }
            if workspace_action_button(ui, "Save", "Save the current PhonoScript script") {
                self.save_phont();
            }
            let script_has_errors = self
                .phont_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error);
            if ui
                .add_enabled_ui(!script_has_errors, |ui| {
                    ui.add_sized([88.0, 28.0], egui::Button::new("Run"))
                })
                .inner
                .on_disabled_hover_text("Resolve the listed PhonoScript errors before running")
                .clicked()
            {
                self.run_phont();
            }
            if workspace_action_button(ui, "Reference", "Show the PhonoScript command reference") {
                self.phont_output.extend(phonoscript_reference());
            }
            ui.separator();
            let name = self
                .phont_path
                .as_ref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("Untitled.phont");
            let script_name = if self.phont_dirty {
                format!("{name} — Edited")
            } else {
                name.to_owned()
            };
            ui.label(
                RichText::new(truncate(&script_name, 42))
                    .small()
                    .color(MUTED),
            )
            .on_hover_text(script_name);
        });
        ui.add_space(8.0);
        let available_height = ui
            .available_height()
            .min(ui.clip_rect().height())
            .max(260.0);
        let has_problems = !self.phont_diagnostics.is_empty();
        let reserved = if has_problems { 240.0 } else { 130.0 };
        let editor_fraction = if has_problems { 0.50 } else { 0.62 };
        let editor_height = (available_height * editor_fraction)
            .max(120.0)
            .min((available_height - reserved).max(120.0));
        let problems_height = if has_problems {
            (available_height * 0.16).clamp(64.0, 140.0)
        } else {
            0.0
        };
        let output_height = (available_height - editor_height - problems_height - 120.0)
            .max(72.0)
            .min((available_height * 0.28).max(72.0));
        let active_source_name = phont_source_name(self.phont_path.as_deref());
        let diagnostic_spans = phonoscript_editor::diagnostic_spans_for_source(
            &self.phont_diagnostics,
            &active_source_name,
        );
        theme::section().show(ui, |ui| {
            let response = phonoscript_source_editor(
                ui,
                &mut self.phont_source,
                editor_height,
                &diagnostic_spans,
            )
            .inner;
            if response.changed() {
                self.phont_dirty = true;
                self.refresh_phont_diagnostics();
            }
        });
        if has_problems {
            ui.add_space(7.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("PROBLEMS").small().strong().color(MUTED));
                let errors = self
                    .phont_diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.severity == Severity::Error)
                    .count();
                let warnings = self.phont_diagnostics.len().saturating_sub(errors);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        counted(errors, "error", "errors"),
                        counted(warnings, "warning", "warnings")
                    ))
                    .small()
                    .color(if errors > 0 { NEGATIVE } else { CAUTION }),
                );
            });
            egui::ScrollArea::vertical()
                .id_salt("phonoscript-problems")
                .max_height(problems_height)
                .show(ui, |ui| {
                    for diagnostic in &self.phont_diagnostics {
                        let color = if diagnostic.severity == Severity::Error {
                            NEGATIVE
                        } else {
                            CAUTION
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&diagnostic.code).monospace().color(color));
                            ui.label(
                                RichText::new(diagnostic_location(diagnostic))
                                    .monospace()
                                    .color(MUTED),
                            );
                            ui.label(&diagnostic.message);
                        });
                        if let Some(help) = &diagnostic.help {
                            ui.indent("phonoscript-problem-help", |ui| {
                                ui.label(RichText::new(help).small().color(MUTED));
                            });
                        }
                    }
                });
        }
        ui.add_space(7.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("OUTPUT").small().strong().color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Clear").clicked() {
                    self.phont_output.clear();
                }
            });
        });
        egui::ScrollArea::both()
            .id_salt("phonoscript-output")
            .stick_to_bottom(true)
            .max_height(output_height)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                for line in self.phont_output.iter().rev().take(120).rev() {
                    ui.label(RichText::new(line).monospace().size(11.5));
                }
            });
    }

    fn paint_plot(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter();
        let chart = rect.shrink2(Vec2::new(48.0, 42.0));
        painter.line_segment(
            [chart.left_bottom(), chart.right_bottom()],
            Stroke::new(1.0_f32, LINE),
        );
        painter.line_segment(
            [chart.left_bottom(), chart.left_top()],
            Stroke::new(1.0_f32, LINE),
        );
        let kind = self.active_evaluator();
        let Ok(evaluation) =
            PhonologicalEngine::new().evaluate(self.active(), kind, self.active_temperature())
        else {
            return;
        };
        let values: Vec<(String, f64)> = match self.document.plot {
            PlotKind::CandidateScores if kind == EvaluatorKind::Ot => evaluation
                .rows
                .iter()
                .map(|row| {
                    let tier = evaluation
                        .ordered_strata
                        .iter()
                        .position(|candidates| candidates.contains(&row.candidate))
                        .map(|index| index + 1)
                        .unwrap_or(0);
                    (
                        self.active().candidates[row.candidate].name.clone(),
                        tier as f64,
                    )
                })
                .collect(),
            PlotKind::CandidateScores => evaluation
                .rows
                .iter()
                .map(|row| {
                    (
                        self.active().candidates[row.candidate].name.clone(),
                        row.harmony,
                    )
                })
                .collect(),
            PlotKind::CandidateProbabilities => evaluation
                .rows
                .iter()
                .map(|row| {
                    (
                        self.active().candidates[row.candidate].name.clone(),
                        row.probability.unwrap_or(0.0),
                    )
                })
                .collect(),
            PlotKind::ConstraintWeights => self
                .active()
                .constraints
                .iter()
                .filter_map(|constraint| {
                    constraint.weight.as_ref().and_then(|weight| {
                        weight
                            .to_f64_center()
                            .ok()
                            .map(|weight| (constraint.name.clone(), weight))
                    })
                })
                .collect(),
            PlotKind::SerialPath => self
                .document
                .serial
                .moves
                .iter()
                .enumerate()
                .map(|(index, item)| (format!("{}→{}", item.from, item.to), index as f64 + 1.0))
                .collect(),
            PlotKind::RankingShares => PhonologicalEngine::new()
                .q_clone_audit(
                    self.active(),
                    self.document.clone_constraint,
                    &self.document.a_priori_rankings,
                    self.active_evaluator(),
                    self.active_temperature(),
                )
                .map(|result| {
                    result
                        .shifts
                        .into_iter()
                        .map(|shift| (shift.answer.join(";"), shift.before.to_f64()))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let maximum = values
            .iter()
            .map(|(_, value)| *value)
            .fold(0.0_f64, f64::max)
            .max(f64::MIN_POSITIVE);
        let slot = chart.width() / values.len().max(1) as f32;
        for (index, (label, value)) in values.iter().enumerate() {
            let width = (slot * 0.58).clamp(8.0, 72.0);
            let height = chart.height() * (*value / maximum).clamp(0.0, 1.0) as f32;
            let center = chart.left() + slot * (index as f32 + 0.5);
            let bar = egui::Rect::from_min_max(
                egui::pos2(center - width / 2.0, chart.bottom() - height),
                egui::pos2(center + width / 2.0, chart.bottom()),
            );
            painter.rect_filled(bar, 1.0, FOCUS);
            painter.text(
                egui::pos2(center, bar.top() - 6.0),
                egui::Align2::CENTER_BOTTOM,
                format!("{value:.3}"),
                FontId::proportional(11.0),
                INK,
            );
            painter.text(
                egui::pos2(center, chart.bottom() + 8.0),
                egui::Align2::CENTER_TOP,
                truncate(label, 14),
                FontId::proportional(10.0),
                MUTED,
            );
        }
    }

    fn add_candidate(&mut self) {
        let width = self.active().constraints.len();
        let number = self.active().candidates.len() + 1;
        let id = next_stable_id(
            "candidate",
            self.active()
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str()),
        );
        self.active_mut().candidates.push(Candidate {
            id,
            name: format!("candidate {number}"),
            form: format!("candidate {number}"),
            violations: vec![UNSET_VIOLATION; width],
            base_mass: NumericScalar::integer(1),
            notes: String::new(),
            observed_frequency: NumericScalar::integer(0),
            structured: None,
        });
        self.selected_candidate = number - 1;
        self.selected_violation = None;
        self.mark_changed();
        self.status =
            "Candidate added; enter every unset violation count before evaluation.".to_owned();
    }

    fn duplicate_candidate(&mut self) {
        let id = next_stable_id(
            "candidate",
            self.active()
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str()),
        );
        if let Some(mut candidate) = self
            .active()
            .candidates
            .get(self.selected_candidate)
            .cloned()
        {
            candidate.id = id;
            candidate.name.push_str(" copy");
            self.active_mut().candidates.push(candidate);
            self.selected_candidate = self.active().candidates.len() - 1;
            self.selected_violation = None;
            self.mark_changed();
        }
    }

    fn remove_candidate(&mut self) {
        if self.active().candidates.len() <= 1 {
            self.report_error("a tableau must retain at least one candidate".to_owned());
            return;
        }
        let index = self
            .selected_candidate
            .min(self.active().candidates.len().saturating_sub(1));
        self.active_mut().candidates.remove(index);
        self.selected_candidate = index.min(self.active().candidates.len() - 1);
        self.selected_violation = None;
        self.mark_changed();
    }

    fn add_constraint(&mut self) {
        let number = self.active().constraints.len() + 1;
        let id = next_stable_id(
            "constraint",
            self.active()
                .constraints
                .iter()
                .map(|constraint| constraint.id.as_str()),
        );
        self.active_mut().constraints.push(Constraint {
            id,
            name: format!("C{number}"),
            weight: Some(NumericScalar::integer(1)),
            stratum: number - 1,
            enabled: true,
            definition: String::new(),
            prior_mean: NumericScalar::integer(0),
            prior_sigma: NumericScalar::integer(100_000),
        });
        for candidate in &mut self.active_mut().candidates {
            candidate.violations.push(UNSET_VIOLATION);
        }
        self.active_mut().normalize();
        self.selected_constraint = number - 1;
        self.selected_violation = None;
        self.mark_changed();
        self.status =
            "Constraint added; enter every unset violation count before evaluation.".to_owned();
    }

    fn remove_constraint(&mut self) {
        if self.active().constraints.len() <= 1 {
            self.report_error("a tableau must retain at least one constraint".to_owned());
            return;
        }
        let index = self
            .selected_constraint
            .min(self.active().constraints.len().saturating_sub(1));
        self.active_mut().constraints.remove(index);
        for candidate in &mut self.active_mut().candidates {
            candidate.violations.remove(index);
        }
        self.selected_constraint = index.min(self.active().constraints.len() - 1);
        self.selected_violation = None;
        self.mark_changed();
    }

    fn evaluate_now(&mut self) {
        let started = Instant::now();
        let kind = self.active_evaluator();
        let result =
            PhonologicalEngine::new().evaluate(self.active(), kind, self.active_temperature());
        self.last_evaluation = started.elapsed();
        let result = match result {
            Ok(result) => result,
            Err(problem) => {
                self.report_error(problem.to_string());
                return;
            }
        };
        let detail = if kind == EvaluatorKind::MaxEnt {
            result
                .rows
                .iter()
                .map(|row| {
                    format!(
                        "{}={:.6}",
                        self.active().candidates[row.candidate].name,
                        row.probability.unwrap_or(0.0)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            result
                .winner_indices
                .iter()
                .map(|index| self.active().candidates[*index].name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        self.status = format!("Evaluated in {}", speed(self.last_evaluation));
        self.console.push(format!("evaluate · {detail}"));
    }

    fn infer_ranking(&mut self) {
        let started = Instant::now();
        match PhonologicalEngine::new()
            .infer_ranking(&self.document.dataset, &self.document.a_priori_rankings)
        {
            Ok(result) => {
                let names: Vec<String> = result
                    .order
                    .iter()
                    .map(|index| self.document.dataset[0].constraints[*index].name.clone())
                    .collect();
                for (stratum, index) in result.order.iter().enumerate() {
                    for tableau in &mut self.document.dataset {
                        tableau.constraints[*index].stratum = stratum;
                    }
                }
                self.dirty = true;
                self.last_evaluation = started.elapsed();
                self.diagnostics = vec![
                    format!("compatible ranking: {}", names.join(" > ")),
                    counted(result.explored_states, "explored state", "explored states"),
                    format!("elapsed: {}", speed(self.last_evaluation)),
                ];
                self.console
                    .push(format!("infer ranking · {}", names.join(" > ")));
            }
            Err(error) => self.report_error(error.to_string()),
        }
    }

    fn learn_weights(&mut self) {
        let started = Instant::now();
        match PhonologicalEngine::new().learn_maxent(
            &self.document.dataset,
            self.document
                .temperature
                .to_f64_center()
                .unwrap_or(f64::NAN),
            10_000,
        ) {
            Ok(result) => {
                for tableau in &mut self.document.dataset {
                    for (constraint, weight) in tableau.constraints.iter_mut().zip(&result.weights)
                    {
                        constraint.weight = Some(
                            NumericScalar::gui_approximate(*weight)
                                .expect("learning returns finite weights"),
                        );
                    }
                }
                self.document.evaluator = EvaluatorKind::MaxEnt;
                self.dirty = true;
                self.last_evaluation = started.elapsed();
                self.diagnostics = vec![
                    format!("learned weights: {:?}", result.weights),
                    counted(result.iterations, "iteration", "iterations"),
                    format!("converged: {}", result.converged),
                    format!(
                        "negative log likelihood: {:.9}",
                        result.negative_log_likelihood
                    ),
                    format!("maximum gradient: {:.3e}", result.maximum_gradient),
                    format!("elapsed: {}", speed(self.last_evaluation)),
                ];
                self.console.push(format!(
                    "learn maxent · {} · NLL {:.9}",
                    counted(result.iterations, "iteration", "iterations"),
                    result.negative_log_likelihood
                ));
            }
            Err(error) => self.report_error(error.to_string()),
        }
    }

    fn run_diagnostics(&mut self) {
        let started = Instant::now();
        let engine = PhonologicalEngine::new();
        let bounds = engine.harmonic_bounds(&self.document.dataset);
        let unnecessary = engine
            .unnecessary_constraints(&self.document.dataset, &self.document.a_priori_rankings);
        let mut lines = Vec::new();
        match bounds {
            Ok(bounds) if bounds.is_empty() => lines.push(
                "harmonic bounding: no observed winner is bounded by a declared loser".to_owned(),
            ),
            Ok(bounds) => {
                for bound in bounds {
                    lines.push(format!(
                        "harmonic bound: {} · {} bounded by {}",
                        bound.input, bound.observed, bound.bounding_rival
                    ));
                }
            }
            Err(error) => lines.push(format!("harmonic-bounding diagnostic refused: {error}")),
        }
        match unnecessary {
            Ok(indices) if indices.is_empty() => {
                lines.push(
                    "constraint necessity: every constraint is individually necessary".to_owned(),
                );
            }
            Ok(indices) => {
                let names: Vec<String> = indices
                    .iter()
                    .map(|index| self.document.dataset[0].constraints[*index].name.clone())
                    .collect();
                lines.push(format!(
                    "individually unnecessary under winner recovery: {}",
                    names.join(", ")
                ));
            }
            Err(error) => lines.push(format!("necessity diagnostic refused: {error}")),
        }
        self.last_evaluation = started.elapsed();
        lines.push(format!("elapsed: {}", speed(self.last_evaluation)));
        self.diagnostics = lines.clone();
        self.console.extend(lines);
    }

    fn compute_typology(&mut self) {
        let started = Instant::now();
        match PhonologicalEngine::new().q_ranking_space(
            &self.document.dataset,
            &self.document.a_priori_rankings,
            self.document.evaluator,
            self.document
                .temperature
                .to_f64_center()
                .unwrap_or(f64::NAN),
        ) {
            Ok(result) => {
                let implications = ranking_implications(&result);
                self.last_evaluation = started.elapsed();
                let mut lines = vec![
                    counted(
                        result.total_rankings,
                        "compatible ranking",
                        "compatible rankings",
                    ),
                    counted(
                        result.winner_counts.len(),
                        "distinct winner pattern",
                        "distinct winner patterns",
                    ),
                    counted(result.dynamic_states, "dynamic state", "dynamic states"),
                    counted(
                        result.completion_states,
                        "completion state",
                        "completion states",
                    ),
                    format!("declared state budget: {}", result.state_budget),
                ];
                for (answer, count) in result.winner_counts.iter().take(40) {
                    lines.push(format!("{count} · {}", answer.join("; ")));
                }
                if !implications.is_empty() {
                    lines.push("t-order implications:".to_owned());
                    for (antecedent, consequences) in implications.iter().take(40) {
                        lines.push(format!("if {antecedent}, then {}", consequences.join(", ")));
                    }
                }
                lines.push(format!("elapsed: {}", speed(self.last_evaluation)));
                self.diagnostics = lines.clone();
                self.console.extend(lines);
            }
            Err(error) => self.report_error(error.to_string()),
        }
    }

    fn run_command(&mut self) {
        let command = self.command.trim().to_owned();
        self.command.clear();
        if command.is_empty() {
            return;
        }
        self.console.push(format!("> {command}"));
        let mut words = command.split_whitespace();
        let verb = words.next().unwrap_or_default().to_lowercase();
        let arguments: Vec<&str> = words.collect();
        match verb.as_str() {
            "help" => self.console.extend([
                "new | open PATH | save [PATH]".to_owned(),
                "evaluator ot|hg|maxent | evaluate | infer ranking | learn maxent | diagnose | typology".to_owned(),
                "workspace project|tableau|serial|second-order|diagnostics|q|plots|phonoscript"
                    .to_owned(),
                "title TEXT | input TEXT | add candidate NAME | add constraint NAME [WEIGHT]".to_owned(),
                "set weight INDEX VALUE | set stratum INDEX VALUE | set mark ROW COLUMN VALUE | set observed ROW VALUE".to_owned(),
                "query winner|surface|order|probability|support | plot scores|probabilities|weights|serial|shares".to_owned(),
                "export tableau|second-order|plot PATH.svg|png|pdf".to_owned(),
                "run phonoscript PATH.phont".to_owned(),
            ]),
            "new" => self.new_document(),
            "open" if !arguments.is_empty() => self.open_path(Path::new(&arguments.join(" "))),
            "save" => {
                if arguments.is_empty() {
                    self.save();
                } else {
                    self.save_path(Path::new(&arguments.join(" ")));
                }
            }
            "evaluate" => self.evaluate_now(),
            "evaluator" if !arguments.is_empty() => {
                let evaluator = match arguments[0].to_lowercase().as_str() {
                    "ot" => Some(EvaluatorKind::Ot),
                    "hg" => Some(EvaluatorKind::HarmonicGrammar),
                    "maxent" => Some(EvaluatorKind::MaxEnt),
                    _ => None,
                };
                if let Some(evaluator) = evaluator {
                    self.document.evaluator = evaluator;
                    self.mark_changed();
                } else {
                    self.report_error("evaluator must be ot, hg, or maxent".to_owned());
                }
            }
            "workspace" if !arguments.is_empty() => {
                self.workspace = match arguments[0] {
                    "project" => Workspace::Project,
                    "tableau" => Workspace::Tableau,
                    "serial" => Workspace::Serial,
                    "second-order" => Workspace::SecondOrder,
                    "diagnostics" => Workspace::Diagnostics,
                    "q" => Workspace::QCalculus,
                    "plots" => Workspace::Plots,
                    "phonoscript" => Workspace::PhonoScript,
                    _ => {
                        self.report_error("unknown workspace".to_owned());
                        return;
                    }
                };
            }
            "title" if !arguments.is_empty() => {
                self.document.title = arguments.join(" ");
                self.mark_changed();
            }
            "input" => {
                self.active_mut().input = arguments.join(" ");
                self.mark_changed();
            }
            "add" if arguments.first() == Some(&"candidate") => {
                self.add_candidate();
                if arguments.len() > 1 {
                    let name = arguments[1..].join(" ");
                    let index = self.selected_candidate;
                    self.active_mut().candidates[index].name.clone_from(&name);
                    self.active_mut().candidates[index].form = name;
                }
            }
            "add" if arguments.first() == Some(&"constraint") => {
                self.add_constraint();
                if arguments.len() > 1 {
                    let index = self.selected_constraint;
                    self.active_mut().constraints[index].name = arguments[1].to_owned();
                    if let Some(weight) = arguments
                        .get(2)
                        .and_then(|value| NumericScalar::parse_editor(value).ok())
                    {
                        self.active_mut().constraints[index].weight = Some(weight);
                    }
                }
            }
            "set" => self.command_set(&arguments),
            "query" if !arguments.is_empty() => {
                let query = match arguments[0] {
                    "winner" => Some(QueryKind::WinnerSet),
                    "surface" => Some(QueryKind::SurfaceWinnerSet),
                    "order" => Some(QueryKind::CompleteOrder),
                    "probability" => Some(QueryKind::ProbabilityLaw),
                    "support" => Some(QueryKind::CandidateSupport),
                    _ => None,
                };
                if let Some(query) = query {
                    self.document.second_order.query = query;
                    self.workspace = Workspace::SecondOrder;
                    self.mark_changed();
                } else {
                    self.report_error("unknown query".to_owned());
                }
            }
            "plot" if !arguments.is_empty() => {
                let plot = match arguments[0] {
                    "scores" => Some(PlotKind::CandidateScores),
                    "probabilities" => Some(PlotKind::CandidateProbabilities),
                    "weights" => Some(PlotKind::ConstraintWeights),
                    "serial" => Some(PlotKind::SerialPath),
                    "shares" => Some(PlotKind::RankingShares),
                    _ => None,
                };
                if let Some(plot) = plot {
                    self.document.plot = plot;
                    self.workspace = Workspace::Plots;
                } else {
                    self.report_error("unknown plot".to_owned());
                }
            }
            "infer" if arguments.first() == Some(&"ranking") => self.infer_ranking(),
            "learn" if arguments.first() == Some(&"maxent") => self.learn_weights(),
            "diagnose" => self.run_diagnostics(),
            "typology" => self.compute_typology(),
            "run" if arguments.first() == Some(&"phonoscript") && arguments.len() > 1 => {
                let path = PathBuf::from(arguments[1..].join(" "));
                self.open_phont_path(&path);
                self.run_phont();
            }
            "export" if arguments.len() >= 2 => {
                let content = arguments[0];
                let path = PathBuf::from(arguments[1..].join(" "));
                self.export_format = match path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("svg")
                    .to_lowercase()
                    .as_str()
                {
                    "png" => ExportFormat::Png,
                    "pdf" => ExportFormat::Pdf,
                    _ => ExportFormat::Svg,
                };
                self.export_path(content, &path);
            }
            _ => self.report_error("unknown or incomplete command; type help".to_owned()),
        }
    }

    fn command_set(&mut self, arguments: &[&str]) {
        let parse_index = |value: Option<&&str>| {
            value
                .and_then(|value| value.parse::<usize>().ok())
                .and_then(|index| index.checked_sub(1))
        };
        match arguments.first().copied() {
            Some("weight") => {
                if let (Some(index), Some(value)) = (
                    parse_index(arguments.get(1)),
                    arguments
                        .get(2)
                        .and_then(|value| NumericScalar::parse_editor(value).ok()),
                ) && let Some(constraint) = self.active_mut().constraints.get_mut(index)
                    && value.to_f64_center().is_ok_and(|value| value >= 0.0)
                {
                    constraint.weight = Some(value);
                    self.mark_changed();
                    return;
                }
            }
            Some("stratum") => {
                if let (Some(index), Some(value)) = (
                    parse_index(arguments.get(1)),
                    arguments
                        .get(2)
                        .and_then(|value| value.parse::<usize>().ok()),
                ) && let Some(constraint) = self.active_mut().constraints.get_mut(index)
                {
                    constraint.stratum = value.saturating_sub(1);
                    self.mark_changed();
                    return;
                }
            }
            Some("mark") => {
                if let (Some(row), Some(column), Some(value)) = (
                    parse_index(arguments.get(1)),
                    parse_index(arguments.get(2)),
                    arguments
                        .get(3)
                        .and_then(|value| value.parse::<u16>().ok())
                        .filter(|value| *value <= MAX_VIOLATION),
                ) && let Some(candidate) = self.active_mut().candidates.get_mut(row)
                    && let Some(mark) = candidate.violations.get_mut(column)
                {
                    *mark = value;
                    self.mark_changed();
                    return;
                }
            }
            Some("observed") => {
                if let (Some(row), Some(value)) = (
                    parse_index(arguments.get(1)),
                    arguments
                        .get(2)
                        .and_then(|value| NumericScalar::parse_editor(value).ok()),
                ) && let Some(candidate) = self.active_mut().candidates.get_mut(row)
                    && value.to_f64_center().is_ok_and(|value| value >= 0.0)
                {
                    candidate.observed_frequency = value;
                    self.mark_changed();
                    return;
                }
            }
            _ => {}
        }
        self.report_error("invalid set command or index".to_owned());
    }

    fn about(&mut self, context: &egui::Context) {
        if self.show_about {
            egui::Window::new("About ConvalGEN")
                .default_width(420.0)
                .min_width(320.0)
                .max_width(560.0)
                .collapsible(false)
                .resizable(true)
                .open(&mut self.show_about)
                .show(context, |ui| {
                    ui.heading("ConvalGEN");
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label("Professional constraint-grammar analysis");
                    ui.separator();
                    ui.label("Optimality Theory · Harmonic Grammar · Maximum Entropy");
                    ui.label("Serial evaluation · Second-Order Tableaux · Q-Calculus");
                    ui.label("PhonoScript language and transactional interpreter");
                    ui.add_space(8.0);
                    ui.label(RichText::new("Alexandre Menezes Barroso").strong());
                    ui.hyperlink_to("alexandrebarroso.com", "https://alexandrebarroso.com");
                    ui.label("© 2026 Alexandre Menezes Barroso");
                    ui.add_space(4.0);
                    ui.label("Free and open-source software · MIT License");
                });
        }
    }

    fn preferences(&mut self, context: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let mut open = self.show_preferences;
        egui::Window::new("ConvalGEN Preferences")
            .default_width(520.0)
            .min_width(420.0)
            .resizable(true)
            .collapsible(false)
            .open(&mut open)
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("preferences-scroll")
                    .max_height(560.0)
                    .show(ui, |ui| {
                        ui.heading("Workspace");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.show_navigator, "Show navigator");
                            ui.checkbox(&mut self.show_inspector, "Show inspector");
                            ui.checkbox(&mut self.show_console, "Show command console");
                        });
                        ui.label(
                            RichText::new(
                                "Side panels collapse automatically when the window is narrow.",
                            )
                            .small()
                            .color(MUTED),
                        );
                        ui.checkbox(
                            &mut self.document.presentation.compact_rows,
                            "Use compact tableau rows",
                        );
                        ui.separator();
                        ui.heading("Keyboard");
                        egui::ComboBox::from_id_salt("shortcut-profile-preferences")
                            .selected_text(self.shortcut_profile.label())
                            .show_ui(ui, |ui| {
                                for profile in ShortcutProfile::ALL {
                                    ui.selectable_value(
                                        &mut self.shortcut_profile,
                                        profile,
                                        profile.label(),
                                    );
                                }
                            });
                        egui::Grid::new("shortcut-reference")
                            .num_columns(2)
                            .spacing([22.0, 5.0])
                            .show(ui, |ui| {
                                let bindings = if self.shortcut_profile == ShortcutProfile::Laptop {
                                    [
                                        ("Add candidate", "Shift+Cmd+A"),
                                        ("Add constraint", "Shift+Cmd+C"),
                                        ("Clear violation", "Shift+Cmd+Backspace"),
                                        ("Move candidate", "Option+Up / Option+Down"),
                                        ("Move constraint", "Option+Left / Option+Right"),
                                    ]
                                } else {
                                    [
                                        ("Add candidate", "Cmd+Return"),
                                        ("Add constraint", "Shift+Cmd+Return"),
                                        ("Clear violation", "Delete"),
                                        ("Move candidate", "Option+Up / Option+Down"),
                                        ("Move constraint", "Option+Left / Option+Right"),
                                    ]
                                };
                                for (action, shortcut) in bindings {
                                    ui.label(action);
                                    ui.label(RichText::new(shortcut).monospace().color(MUTED));
                                    ui.end_row();
                                }
                            });
                        ui.separator();
                        ui.heading("Export defaults");
                        ui.horizontal_wrapped(|ui| {
                            ui.checkbox(&mut self.document.presentation.show_title, "Title");
                            ui.checkbox(&mut self.document.presentation.show_author, "Author");
                            ui.checkbox(&mut self.document.presentation.show_legend, "Legend");
                        });
                        ui.add(
                            egui::Slider::new(
                                &mut self.document.presentation.export_scale,
                                0.5..=4.0,
                            )
                            .text("Scale"),
                        );
                    });
            });
        self.show_preferences = open;
    }

    fn help(&mut self, context: &egui::Context) {
        if !self.show_help {
            return;
        }
        let mut open = self.show_help;
        egui::Window::new("ConvalGEN Help")
            .default_size([620.0, 620.0])
            .min_size([440.0, 360.0])
            .resizable(true)
            .collapsible(false)
            .open(&mut open)
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Working with a project");
                    ui.label("A .ottab project may contain many tableaux. Use the navigator or the tableau selector to switch between them; Project creates, duplicates, moves, and removes tableaux.");
                    ui.separator();
                    ui.heading("Editing a tableau");
                    ui.label("Add or remove candidates and constraints with the action bars. Click the input, a candidate identity or form, a constraint name, a rank or weight, or an analyst-entered violation to edit it directly. Every violation count is entered by the phonologist: ConvalGEN never infers marks from candidate structure, candidate form, constraint name, or constraint definition. MaxEnt observed counts and prior masses remain input cells; costs and probabilities are derived and read-only. Press Delete to clear the selected editable cell. Move candidates with Option+Up or Option+Down; move constraints and their violation columns with Option+Left or Option+Right. In OT, moving a constraint establishes a strict ranking. Tie left places the selected constraint in the preceding stratum.");
                    ui.separator();
                    ui.heading("Evaluation and ties");
                    ui.label("OT uses lexicographic strict domination by ranked strata. HG minimizes weighted violation cost. MaxEnt converts weighted costs and base masses into a normalized probability law. The tie policy may retain every co-winner, select the first listed candidate, or require a unique winner; an unresolved required-unique tie is reported explicitly.");
                    ui.separator();
                    ui.heading("Second-Order Tableau and Q-Calculus");
                    ui.label("Source and target analyses are evaluated independently. A declared transport aligns their typed answers; it does not calculate the target answer. Missing formation or admission dependencies return a structured not-evaluated result instead of false, NaN, or an empty answer.");
                    ui.separator();
                    ui.heading("PhonoScript");
                    ui.label("PhonoScript (.phont) is ConvalGEN’s transactional scripting language. A script may create projects and tableaux, generate candidates, evaluate OT/HG/MaxEnt and serial analyses, assert results, infer rankings, train MaxEnt weights, run Q-Calculus comparisons, and export figures. A failing command reports its source line and leaves the open project unchanged.");
                    ui.separator();
                    ui.heading("Saving and export");
                    ui.label("Save projects as .ottab. Export current tableaux, Second-Order Tableaux, plots, or complete projects as editable SVG, PNG, or PDF. The native renderer follows the bundled secondordertableau.sty visual specification, crops each figure to its content, and never emits .tex. Export presentation defaults are available under Options → Preferences.");
                });
            });
        self.show_help = open;
    }
}

impl eframe::App for ConvalgenApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        let document_before_frame = self.document.clone();
        let opened_files: Vec<PathBuf> = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .filter(|path| {
                    path.extension().is_some_and(|extension| {
                        extension.eq_ignore_ascii_case(document::EXTENSION)
                            || extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION)
                    })
                })
                .collect()
        });
        #[cfg(target_os = "macos")]
        let opened_files = opened_files
            .into_iter()
            .chain(crate::macos::take_opened_files())
            .collect::<Vec<_>>();
        if let Some(path) = opened_files.last() {
            self.open_path(path);
        }
        self.shortcuts(context);
        self.menu_bar(context);
        self.toolbar(context);
        self.status_bar(context);
        let viewport = context.viewport_rect().size();
        let (console_available, navigator_available, inspector_available) =
            adaptive_panel_availability(viewport);
        if console_available {
            self.console_panel(context);
        }
        if navigator_available {
            self.navigator(context);
        }
        if inspector_available {
            self.inspector(context);
        }
        self.workspace(context);
        self.preferences(context);
        self.help(context);
        self.about(context);
        let window_title = format!("{} — ConvalGEN", self.display_name());
        if window_title != self.last_window_title {
            context.send_viewport_cmd(egui::ViewportCommand::Title(window_title.clone()));
            self.last_window_title = window_title;
        }
        if self.document != document_before_frame {
            if self.history_replaying {
                self.history_replaying = false;
            } else {
                self.undo_stack.push(document_before_frame);
                if self.undo_stack.len() > 128 {
                    self.undo_stack.remove(0);
                }
                self.redo_stack.clear();
            }
        } else {
            self.history_replaying = false;
        }
    }
}

fn assign_strict_ranks_by_column(tableau: &mut Tableau) {
    for (rank, constraint) in tableau.constraints.iter_mut().enumerate() {
        constraint.stratum = rank;
    }
}

fn compact_constraint_strata(tableau: &mut Tableau) {
    let mut strata: Vec<usize> = tableau
        .constraints
        .iter()
        .map(|constraint| constraint.stratum)
        .collect();
    strata.sort_unstable();
    strata.dedup();
    for constraint in &mut tableau.constraints {
        if let Ok(index) = strata.binary_search(&constraint.stratum) {
            constraint.stratum = index;
        }
    }
}

fn action_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> bool {
    ui.add_sized([78.0, 28.0], egui::Button::new(label))
        .on_hover_text(tooltip)
        .clicked()
}

fn toolbar_sized_button(ui: &mut egui::Ui, label: &str, tooltip: &str, width: f32) -> bool {
    ui.add_sized([width, 28.0], egui::Button::new(label))
        .on_hover_text(tooltip)
        .clicked()
}

fn project_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> bool {
    ui.add_sized([154.0, 28.0], egui::Button::new(label))
        .on_hover_text(tooltip)
        .clicked()
}

fn diagnostic_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add_sized([148.0, 28.0], egui::Button::new(label))
        .clicked()
}

fn workspace_action_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> bool {
    ui.add_sized([88.0, 28.0], egui::Button::new(label))
        .on_hover_text(tooltip)
        .clicked()
}

fn selectable_truncated(
    ui: &mut egui::Ui,
    selected: bool,
    value: &str,
    maximum: usize,
) -> egui::Response {
    let displayed = truncate(value, maximum);
    let response = ui.selectable_label(selected, &displayed);
    if displayed == value {
        response
    } else {
        response.on_hover_text(value)
    }
}

fn counted<T>(count: T, singular: &str, plural: &str) -> String
where
    T: std::fmt::Display + PartialEq + From<u8>,
{
    let noun = if count == T::from(1) {
        singular
    } else {
        plural
    };
    format!("{count} {noun}")
}

fn replace_constraint_register(tableau: &mut Tableau, constraints: &[Constraint]) {
    if tableau.constraints == constraints
        && tableau
            .candidates
            .iter()
            .all(|candidate| candidate.violations.len() == constraints.len())
    {
        return;
    }
    tableau.constraints = constraints.to_vec();
    for candidate in &mut tableau.candidates {
        candidate.violations = vec![UNSET_VIOLATION; constraints.len()];
    }
}

fn adaptive_panel_availability(viewport: Vec2) -> (bool, bool, bool) {
    (
        viewport.y >= CONSOLE_BREAKPOINT,
        viewport.x >= NAVIGATOR_BREAKPOINT,
        viewport.x >= INSPECTOR_BREAKPOINT,
    )
}

fn tableau_body_height(row_height: f32, visible_rows: usize, viewport_height: f32) -> f32 {
    let content_height = row_height * visible_rows.max(1) as f32;
    let maximum_height = (viewport_height * 0.55).clamp(row_height * 3.0, row_height * 14.0);
    content_height.min(maximum_height)
}

fn speed(elapsed: Duration) -> String {
    if elapsed.as_micros() < 1_000 {
        format!("{} µs", elapsed.as_micros())
    } else {
        format!("{:.2} ms", elapsed.as_secs_f64() * 1_000.0)
    }
}

fn path_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

fn file_stem(value: &str) -> String {
    let stem = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase();
    if stem.is_empty() {
        "tableau".to_owned()
    } else {
        stem
    }
}

fn phonoscript_reference() -> Vec<String> {
    [
        "PhonoScript 3 · parsed phonological programming language with local modules",
        "let x = 1/3; var total = 0; total = total + x;",
        "fn choose(x) { if x > 0 { return x } return 0 }",
        "export let inventory = [\"p\", \"t\", \"k\"]; export fn analyse(x) { return x }",
        "import { inventory, analyse as run } from \"./grammar.phont\"",
        "Saved entry scripts use their containing directory as the confined module root; save edited imports before running.",
        "for n in range(0, 4) { print(n) }",
        "project_title(\"Title\"); project_author(\"Name\");",
        "tableau_new(\"Name\", \"/input/\"); tableau_select(\"Name\");",
        "constraint_add(\"C1\", 1); candidate_add(\"a\", \"[a]\", [0]);",
        "project_evaluator(\"OT\"); evaluate(); assert_winners([\"a\"]);",
        "project_evaluator(\"HG\"); constraint_weight(\"C1\", 3/2);",
        "project_evaluator(\"MaxEnt\"); probability(\"a\"); maxent_learn(500);",
        "serial_side(\"source\"); serial_start(\"ab\"); serial_evaluate();",
        "second_query(\"winner_set\"); second_compare();",
        "q_ranking_space(); q_clone(\"C1\"); constraint_demotion(\"a\");",
        "generator_delete(\"ab\"); generator_insert(\"ab\", [\"x\"]);",
        "save(\"analysis.ottab\"); export_tableau(\"tableau.svg\");",
        "// and /* */ comments are supported; execution is transactional.",
        "Exact source arithmetic remains rational. Numerical engine crossings are reported as PSR0701 warnings.",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn format_answer(answer: &[Vec<String>]) -> String {
    answer
        .iter()
        .map(|stratum| format!("{{{}}}", stratum.join(", ")))
        .collect::<Vec<_>>()
        .join(" > ")
}

fn format_share(share: &ExactRankingShare) -> String {
    share.to_string()
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        value.to_owned()
    } else {
        let mut result: String = value.chars().take(maximum.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

#[cfg(test)]
mod gui_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct ModuleFixture {
        path: PathBuf,
    }

    impl ModuleFixture {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos();
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/test-modules")
                .join(format!("{label}-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).expect("create confined GUI module fixture");
            Self { path }
        }
    }

    impl Drop for ModuleFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn count_labels_use_english_singular_and_plural_forms() {
        assert_eq!(counted(0_usize, "tableau", "tableaux"), "0 tableaux");
        assert_eq!(counted(1_usize, "tableau", "tableaux"), "1 tableau");
        assert_eq!(counted(2_usize, "tableau", "tableaux"), "2 tableaux");
        for (singular, plural) in [
            ("candidate", "candidates"),
            ("constraint", "constraints"),
            ("error", "errors"),
            ("warning", "warnings"),
            ("coordinate", "coordinates"),
            ("step", "steps"),
        ] {
            assert_eq!(counted(1_usize, singular, plural), format!("1 {singular}"));
            assert_eq!(counted(2_usize, singular, plural), format!("2 {plural}"));
        }
        assert_eq!(
            counted(1_u128, "compatible ranking", "compatible rankings"),
            "1 compatible ranking"
        );
    }

    #[test]
    fn in_app_reference_identifies_phonoscript_three_and_local_modules() {
        let reference = phonoscript_reference();
        assert!(reference[0].starts_with("PhonoScript 3"));
        assert!(reference.iter().any(|line| line.starts_with("import {")));
        assert!(
            reference
                .iter()
                .any(|line| line.contains("confined module root"))
        );
        assert!(!reference.iter().any(|line| line.contains("PhonoScript 2")));
    }

    #[test]
    fn adaptive_panels_leave_compact_workspaces_unobstructed() {
        assert_eq!(
            adaptive_panel_availability(Vec2::new(640.0, 480.0)),
            (false, false, false)
        );
        assert_eq!(
            adaptive_panel_availability(Vec2::new(899.0, 800.0)),
            (true, false, false)
        );
        assert_eq!(
            adaptive_panel_availability(Vec2::new(900.0, 800.0)),
            (true, true, true)
        );
    }

    #[test]
    fn large_tableaux_get_a_bounded_vertical_body() {
        assert_eq!(tableau_body_height(30.0, 1, 400.0), 30.0);
        assert_eq!(tableau_body_height(30.0, 100, 400.0), 220.0);
        assert_eq!(tableau_body_height(30.0, 100, 2_000.0), 420.0);
    }

    #[test]
    fn status_paths_disclose_only_the_leaf_name() {
        let path = Path::new("private/research/analysis.ottab");
        assert_eq!(path_display_name(path), "analysis.ottab");
        assert_eq!(phont_path_event("opened", path), "opened analysis.ottab");
        assert_eq!(phont_path_event("saved", path), "saved analysis.ottab");
    }

    #[test]
    fn compact_phonoscript_editor_exposes_horizontal_overflow_for_long_lines() {
        egui::__run_test_ui(|ui| {
            let mut source = format!("print(\"{}\")\n", "candidate".repeat(40));
            let output = ui
                .allocate_ui_with_layout(
                    Vec2::new(320.0, 200.0),
                    Layout::top_down(Align::Min),
                    |ui| phonoscript_source_editor(ui, &mut source, 180.0, &[]),
                )
                .inner;
            assert!(
                output.content_size.x > output.inner_rect.width(),
                "content {:?}, viewport {:?}, response {:?}",
                output.content_size,
                output.inner_rect.size(),
                output.inner.rect.size()
            );
            assert!(output.inner.rect.width() >= output.content_size.x - 1.0);
        });
    }

    #[test]
    fn unsaved_imports_receive_a_source_anchored_save_diagnostic() {
        let source = "import { winner } from \"./grammar.phont\"\nprint(winner)\n";
        let diagnostic = check_phont_source(source, None, false)
            .into_iter()
            .find(|diagnostic| diagnostic.code == RuntimeDiagnosticCode::ModuleResolution.as_str())
            .expect("unsaved-module diagnostic");
        assert_eq!(diagnostic.source_name, UNTITLED_PHONT_SOURCE);
        assert_eq!(diagnostic.primary.start.byte, 0);
        assert!(diagnostic.message.contains("saved PhonoScript entry file"));
        assert!(
            diagnostic
                .help
                .as_deref()
                .is_some_and(|help| help.contains("containing directory"))
        );

        let dirty = check_phont_source(source, Some(Path::new("project/main.phont")), true);
        assert!(dirty.iter().any(|diagnostic| {
            diagnostic.message.contains("unsaved editor changes")
                && diagnostic.source_name == "main.phont"
        }));
        assert!(!dirty.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("could not resolve entry module")
        }));
    }

    #[test]
    fn saved_modules_check_and_run_under_the_entry_directory() {
        let fixture = ModuleFixture::new("saved-graph");
        let entry = fixture.path.join("main.phont");
        let library = fixture.path.join("library.phont");
        let entry_source = "import { answer } from \"./library.phont\"\nprint(answer)\n";
        fs::write(&entry, entry_source).expect("write entry module");
        fs::write(&library, "export let answer = 7\n").expect("write library module");

        let diagnostics = check_phont_source(entry_source, Some(&entry), false);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != Severity::Error),
            "{diagnostics:#?}"
        );
        let result = run_phont_source(
            entry_source,
            Some(&entry),
            false,
            &ConvalgenDocument::blank(),
        )
        .expect("module graph admitted");
        assert!(result.succeeded(), "{:#?}", result.diagnostics);
        assert_eq!(result.standard_output, vec!["7"]);
        assert_eq!(result.statistics.modules_loaded, 1);

        fs::write(&library, "export let answer = @\n").expect("write invalid library module");
        let diagnostics = check_phont_source(entry_source, Some(&entry), false);
        let imported = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.source_name == "library.phont")
            .expect("imported-file diagnostic retains its relative source path");
        assert_eq!(
            diagnostic_location(imported),
            format!(
                "library.phont:{}:{}",
                imported.primary.start.line, imported.primary.start.column
            )
        );
        let entry_overlays =
            phonoscript_editor::diagnostic_spans_for_source(&diagnostics, "main.phont");
        assert_eq!(
            entry_overlays.len(),
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.source_name == "main.phont")
                .count()
        );
    }

    #[test]
    fn module_root_confines_imports_and_single_file_buffers_still_run() {
        let fixture = ModuleFixture::new("confined-root");
        let project = fixture.path.join("project");
        fs::create_dir_all(&project).expect("create module root");
        let entry = project.join("main.phont");
        fs::write(fixture.path.join("outside.phont"), "export let value = 1\n")
            .expect("write outside module");
        let escaping = "import { value } from \"../outside.phont\"\nprint(value)\n";
        fs::write(&entry, escaping).expect("write escaping entry");
        let diagnostics = check_phont_source(escaping, Some(&entry), false);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == RuntimeDiagnosticCode::ModuleResolution.as_str()
                && diagnostic
                    .message
                    .contains("outside the declared module root")
        }));

        let single = run_phont_source(
            "let value = 3\nprint(value)\n",
            None,
            true,
            &ConvalgenDocument::blank(),
        )
        .expect("module-free editor buffer remains executable");
        assert!(single.succeeded());
        assert_eq!(single.standard_output, vec!["3"]);
        assert_eq!(single.statistics.modules_loaded, 0);
    }

    #[test]
    fn replacing_constraint_register_never_reinterprets_old_marks() {
        let mut tableau = ConvalgenDocument::blank().dataset.remove(0);
        tableau.candidates[0].violations[0] = 3;
        let retained = tableau.constraints[0].clone();
        replace_constraint_register(&mut tableau, std::slice::from_ref(&retained));
        assert_eq!(tableau.candidates[0].violations, vec![3]);
        let added = Constraint {
            id: "markedness".to_owned(),
            name: "*M".to_owned(),
            weight: Some(NumericScalar::integer(1)),
            stratum: 0,
            enabled: true,
            definition: String::new(),
            prior_mean: NumericScalar::integer(0),
            prior_sigma: NumericScalar::integer(100_000),
        };
        replace_constraint_register(&mut tableau, &[added, retained]);
        assert_eq!(
            tableau.candidates[0].violations,
            vec![UNSET_VIOLATION, UNSET_VIOLATION]
        );
    }
}
