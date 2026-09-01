//! Transactional execution for PhonoScript.
//!
//! The parser preserves exact source numbers and this runtime keeps them as
//! rational values until a checked boundary into the floating-point
//! phonological engine.  One execution owns a private document clone.  The
//! clone and any queued file effects are committed only after every statement
//! has completed successfully.

// Runtime faults deliberately retain a stable code, complete source span,
// call stack, and remediation context. Boxing that public diagnostic would
// make every interpreter path less direct without reducing its bounded data.
#![allow(clippy::result_large_err)]
// The focused runtime regression module sits beside the interpreter core it
// exercises; domain-specific builtin implementations follow it in the same
// file. Keeping that locality is clearer than moving a large test module to
// the end of this intentionally single-file runtime.
#![allow(clippy::items_after_test_module)]

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use num_bigint::{BigInt, BigUint};
use num_rational::BigRational;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Serialize, de::DeserializeOwned};

use crate::document;
use crate::engine::{ComparisonStatus, TableauEvaluation};
use crate::exact::{ApproximationBoundary, ApproximationMethod, NumericScalar};
use crate::export::{self, ExportFormat};
use crate::model::{
    Candidate, ComparisonMode, Constraint, ConsumerMode, ConvalgenDocument, DOCUMENT_VERSION,
    DependencyScope, DependencyStage, EvaluatorKind, MAX_VIOLATION, MissingDependency,
    NormalizerPolicy, QueryKind, ResponseDomain, SecondOrderLayout, SerialMove, SerialSettings,
    Tableau, TiePolicy, next_stable_id,
};
use crate::phonological_engine::{EngineError, PhonologicalEngine};
use crate::phonology::{
    AssociationTarget, AutosegmentalTier, CompletenessStatus, FeatureName, FeatureValue,
    FiniteGenerator, Foot, FootId, GenerationResult, GeneratorSpec, Mora, MoraId, Morpheme,
    MorphemeId, MorphemeKind, ProsodicStructure, ProsodicWord, ProsodicWordId, SegmentId,
    SegmentTemplate, StressLevel, StructuredCandidate, Syllable, SyllableId, TierAssociation,
    TierNode, TierNodeId, TierValue, ToneValue, UnderlyingForm,
};
use crate::phonoscript_analysis;
use crate::phonoscript_frontend::{
    self as frontend, BinaryOperator, Expression, ExpressionKind, Literal, NumericLiteral,
    RelatedSpan, Severity, Span, Statement, StatementKind, UnaryOperator,
};
use crate::ranking::{ConstraintDemotionResult, LinearExtensions, MarkData, PartialRanking};

// ---------------------------------------------------------------------------
// Public values, limits, and diagnostics

/// Current source-language generation. This is intentionally independent of
/// the `.ottab` document-format version.
pub const LANGUAGE_VERSION: u32 = 3;

/// Conventional extension for PhonoScript source files.
pub const EXTENSION: &str = "phont";

/// Source identity used by the compatibility entry points that receive source
/// text without a filename. File-aware callers should prefer [`check_named`],
/// [`run_named`], or [`run_with_limits_named`].
pub const ANONYMOUS_SOURCE_NAME: &str = "<memory>";

/// Builtins that can lower an exact script number into the current f64-backed
/// model or numerical assertion layer. Every such lowering emits PSR0701 and
/// appends a [`BoundaryConversion`] record.
pub const APPROXIMATE_ENGINE_BOUNDARY_BUILTINS: &[&str] = &["assert_approx", "assert_probability"];

/// Emit a complete project as executable PhonoScript. The payload is the
/// canonical current `.ottab` document nested in a source string, so fields that do
/// not have a convenient interactive setter are still preserved losslessly.
/// The generated script is deterministic for a normalized document.
pub fn emit(document: &ConvalgenDocument) -> String {
    match try_emit(document) {
        Ok(source) => source,
        Err(message) => format!(
            "// PhonoScript v{LANGUAGE_VERSION} emission failed.\nassert(false, {});\n",
            quote_source_text(&message)
        ),
    }
}

/// Checked counterpart to [`emit`] for callers that need to report an invalid
/// document rather than receiving an executable failing assertion.
pub fn try_emit(document: &ConvalgenDocument) -> Result<String, String> {
    let encoded = document::encode(document)?;
    let json = String::from_utf8(encoded)
        .map_err(|error| format!("encoded document was not UTF-8: {error}"))?;
    Ok(format!(
        "// PhonoScript v{LANGUAGE_VERSION}; embedded .ottab v{DOCUMENT_VERSION} model preserves exact and explicitly approximate scalars.\nproject_restore_v2({});\n",
        quote_source_text(&json)
    ))
}

fn quote_source_text(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len().saturating_add(2));
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\0' => quoted.push_str("\\0"),
            value if value.is_control() => {
                quoted.push_str(&format!("\\u{{{:x}}}", u32::from(value)));
            }
            value => quoted.push(value),
        }
    }
    quoted.push('"');
    quoted
}

/// A script number is either exact or explicitly marked as an approximation
/// returned by a numerical engine boundary.  Exact and approximate values are
/// never silently conflated in rendered output.
#[derive(Debug, Clone)]
pub enum Number {
    Exact(BigRational),
    Approximate(f64),
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(left), Self::Exact(right)) => left == right,
            (Self::Approximate(left), Self::Approximate(right)) => left.total_cmp(right).is_eq(),
            // Representation boundaries are semantic: approximate center
            // equality is not an exact equality certificate.
            (Self::Exact(_), Self::Approximate(_)) | (Self::Approximate(_), Self::Exact(_)) => {
                false
            }
        }
    }
}

impl Number {
    fn exact(value: impl Into<BigInt>) -> Self {
        Self::Exact(BigRational::from_integer(value.into()))
    }

    fn is_zero(&self) -> bool {
        match self {
            Self::Exact(value) => value.is_zero(),
            Self::Approximate(value) => *value == 0.0,
        }
    }

    fn finite_approximate(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self::Approximate(value))
    }
}

impl Display for Number {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(value) if value.is_integer() => write!(formatter, "{}", value.to_integer()),
            Self::Exact(value) => write!(formatter, "{}/{}", value.numer(), value.denom()),
            Self::Approximate(value) => write!(formatter, "~{value:.12}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(Number),
    Boolean(bool),
    Text(String),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
    Null,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Number(Number::Exact(_)) => "exact-number",
            Self::Number(Number::Approximate(_)) => "approximate-number",
            Self::Boolean(_) => "boolean",
            Self::Text(_) => "text",
            Self::List(_) => "list",
            Self::Record(_) => "record",
            Self::Null => "null",
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Number(value) => value.to_string(),
            Self::Boolean(value) => value.to_string(),
            Self::Text(value) => value.clone(),
            Self::List(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Record(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Null => "null".to_owned(),
        }
    }
}

impl Display for Value {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeDiagnosticCode {
    Frontend,
    ModuleResolution,
    ModuleCycle,
    ModuleLimit,
    ModuleExport,
    ModuleDeclaration,
    StepLimit,
    LoopLimit,
    CallDepth,
    CollectionLimit,
    NumericLimit,
    UndefinedName,
    DuplicateName,
    ImmutableAssignment,
    NotCallable,
    Arity,
    Type,
    DivisionByZero,
    InvalidIndex,
    ReturnOutsideFunction,
    UnsupportedCommand,
    AssertionFailed,
    DomainFormation,
    DomainBoundary,
    EngineRefusal,
    FileEffect,
    ApproximateBoundary,
    InternalState,
}

impl RuntimeDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Frontend => "PSR0001",
            Self::ModuleResolution => "PSR0101",
            Self::ModuleCycle => "PSR0102",
            Self::ModuleLimit => "PSR0103",
            Self::ModuleExport => "PSR0104",
            Self::ModuleDeclaration => "PSR0105",
            Self::StepLimit => "PSR0201",
            Self::LoopLimit => "PSR0202",
            Self::CallDepth => "PSR0203",
            Self::CollectionLimit => "PSR0204",
            Self::NumericLimit => "PSR0205",
            Self::UndefinedName => "PSR0301",
            Self::DuplicateName => "PSR0302",
            Self::ImmutableAssignment => "PSR0303",
            Self::NotCallable => "PSR0304",
            Self::Arity => "PSR0305",
            Self::Type => "PSR0401",
            Self::DivisionByZero => "PSR0402",
            Self::InvalidIndex => "PSR0403",
            Self::ReturnOutsideFunction => "PSR0404",
            Self::UnsupportedCommand => "PSR0405",
            Self::AssertionFailed => "PSR0450",
            Self::DomainFormation => "PSR0501",
            Self::DomainBoundary => "PSR0502",
            Self::EngineRefusal => "PSR0503",
            Self::FileEffect => "PSR0601",
            Self::ApproximateBoundary => "PSR0701",
            Self::InternalState => "PSR0901",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSite {
    pub function: String,
    pub source_name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDiagnostic {
    pub source_name: String,
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub primary: Span,
    pub related: Vec<RelatedSpan>,
    pub help: Option<String>,
    pub call_stack: Vec<CallSite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLimits {
    /// Maximum number of distinct canonical `.phont` files, including the
    /// entry module, admitted into one local module graph.
    pub maximum_modules: usize,
    /// Maximum import-edge depth below the entry module.
    pub maximum_module_depth: usize,
    /// Maximum aggregate UTF-8 source bytes read for one module graph. Because
    /// every individual source contributes to the aggregate, this is also a
    /// hard bound on each single source file.
    pub maximum_module_source_bytes: usize,
    pub maximum_steps: u64,
    pub maximum_loop_iterations: u64,
    pub maximum_call_depth: usize,
    pub maximum_collection_items: usize,
    pub maximum_exact_bytes: usize,
    pub maximum_output_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            maximum_modules: 256,
            maximum_module_depth: 64,
            maximum_module_source_bytes: 16 * 1024 * 1024,
            maximum_steps: 1_000_000,
            maximum_loop_iterations: 100_000,
            maximum_call_depth: 256,
            maximum_collection_items: 250_000,
            maximum_exact_bytes: 1_000_000,
            maximum_output_bytes: 4_000_000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeStatistics {
    /// Number of distinct imported canonical modules whose declarations were
    /// evaluated. The entry file is not counted.
    pub modules_loaded: usize,
    pub steps: u64,
    pub statements: u64,
    pub expressions: u64,
    pub calls: u64,
    pub loop_iterations: u64,
    pub engine_calls: u64,
    pub exact_to_engine_conversions: u64,
    pub queued_file_effects: usize,
}

/// Lossless provenance for a script literal crossing into the present f64
/// engine model. `exact_value` is retained for a future exact-model lowering;
/// `engine_value` is the finite binary value actually supplied today.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryConversion {
    pub coordinate: String,
    pub exact_value: BigRational,
    pub engine_value: f64,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub document: ConvalgenDocument,
    pub committed: bool,
    pub value: Value,
    pub standard_output: Vec<String>,
    pub diagnostics: Vec<RuntimeDiagnostic>,
    pub boundary_conversions: Vec<BoundaryConversion>,
    pub selected_tableau: SelectedTableau,
    pub statistics: RuntimeStatistics,
}

impl RunResult {
    pub fn succeeded(&self) -> bool {
        self.committed
            && !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Tableau selected by the script when execution stopped. A committed result
/// can be fed directly to a GUI; on rollback this identifies the script's last
/// attempted selection without changing the returned document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTableau {
    Source,
    Target,
    Dataset(usize),
}

// ---------------------------------------------------------------------------
// Runtime environment

type EnvironmentRef = Rc<RefCell<Environment>>;

#[derive(Clone)]
struct UserFunction {
    name: String,
    source_name: String,
    parameters: Vec<String>,
    body: Vec<Statement>,
    closure: Weak<RefCell<Environment>>,
}

#[derive(Clone)]
enum BindingValue {
    Data(Value),
    Function(UserFunction),
    Builtin(&'static str),
}

#[derive(Clone)]
struct Binding {
    mutable: bool,
    value: BindingValue,
}

struct Environment {
    parent: Option<EnvironmentRef>,
    bindings: HashMap<String, Binding>,
}

impl Environment {
    fn root() -> EnvironmentRef {
        Rc::new(RefCell::new(Self {
            parent: None,
            bindings: HashMap::new(),
        }))
    }

    fn child(parent: EnvironmentRef) -> EnvironmentRef {
        Rc::new(RefCell::new(Self {
            parent: Some(parent),
            bindings: HashMap::new(),
        }))
    }
}

#[derive(Debug, Clone)]
struct RuntimeFault {
    code: RuntimeDiagnosticCode,
    message: String,
    source_name: Option<String>,
    span: Span,
    help: Option<String>,
    call_stack: Vec<CallSite>,
}

impl RuntimeFault {
    fn new(code: RuntimeDiagnosticCode, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source_name: None,
            span,
            help: None,
            call_stack: Vec::new(),
        }
    }

    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn at_source(mut self, source_name: impl Into<String>) -> Self {
        if self.source_name.is_none() {
            self.source_name = Some(source_name.into());
        }
        self
    }
}

enum Flow {
    Continue(Value),
    Return(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableauSlot {
    Source,
    Target,
    Dataset(usize),
}

impl From<TableauSlot> for SelectedTableau {
    fn from(value: TableauSlot) -> Self {
        match value {
            TableauSlot::Source => Self::Source,
            TableauSlot::Target => Self::Target,
            TableauSlot::Dataset(index) => Self::Dataset(index),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SerialSide {
    Source,
    Target,
}

enum PendingEffect {
    Save {
        path: PathBuf,
        document: Box<ConvalgenDocument>,
    },
    Export {
        path: PathBuf,
        format: ExportFormat,
        scale: f32,
        svg: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModuleEdgeKey {
    importer: PathBuf,
    import_byte: usize,
}

#[derive(Clone)]
struct ModuleUnit {
    source_name: String,
    program: frontend::Program,
    export_names: Vec<String>,
}

struct PreparedModuleGraph {
    entry: PathBuf,
    units: HashMap<PathBuf, ModuleUnit>,
    edges: HashMap<ModuleEdgeKey, PathBuf>,
}

struct LoadedModule {
    // User functions retain their lexical environment weakly; the module
    // cache owns it strongly for the rest of the transaction.
    _environment: EnvironmentRef,
    exports: HashMap<String, Binding>,
}

struct Runtime {
    source_name: String,
    current_module: Option<PathBuf>,
    initial: ConvalgenDocument,
    document: ConvalgenDocument,
    engine: PhonologicalEngine,
    limits: RuntimeLimits,
    statistics: RuntimeStatistics,
    output: Vec<String>,
    output_bytes: usize,
    call_stack: Vec<CallSite>,
    selected_tableau: TableauSlot,
    serial_side: SerialSide,
    effects: Vec<PendingEffect>,
    warnings: Vec<RuntimeDiagnostic>,
    boundary_conversions: Vec<BoundaryConversion>,
    generation_results: BTreeMap<u64, GenerationResult>,
    next_generation_handle: u64,
    last_value: Value,
    builtin_environment: EnvironmentRef,
    module_graph: Option<PreparedModuleGraph>,
    loaded_modules: HashMap<PathBuf, LoadedModule>,
    loading_modules: HashSet<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleVisitState {
    Visiting,
    Complete,
}

#[derive(Clone)]
struct ImportFrame {
    source_name: String,
    span: Span,
}

struct ModuleGraphBuilder {
    root: PathBuf,
    entry: PathBuf,
    limits: RuntimeLimits,
    states: HashMap<PathBuf, ModuleVisitState>,
    units: HashMap<PathBuf, ModuleUnit>,
    edges: HashMap<ModuleEdgeKey, PathBuf>,
    diagnostics: Vec<RuntimeDiagnostic>,
    module_stack: Vec<PathBuf>,
    import_stack: Vec<ImportFrame>,
    source_bytes: usize,
}

impl ModuleGraphBuilder {
    fn new(root: PathBuf, entry: PathBuf, limits: RuntimeLimits) -> Self {
        Self {
            root,
            entry,
            limits,
            states: HashMap::new(),
            units: HashMap::new(),
            edges: HashMap::new(),
            diagnostics: Vec::new(),
            module_stack: Vec::new(),
            import_stack: Vec::new(),
            source_bytes: 0,
        }
    }

    fn source_name(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn diagnostic(
        &mut self,
        source_name: impl Into<String>,
        code: RuntimeDiagnosticCode,
        span: Span,
        message: impl Into<String>,
        help: Option<String>,
        call_stack: Vec<CallSite>,
    ) {
        self.diagnostics.push(RuntimeDiagnostic {
            source_name: source_name.into(),
            code: code.as_str().to_owned(),
            severity: Severity::Error,
            message: message.into(),
            primary: span,
            related: Vec::new(),
            help,
            call_stack,
        });
    }

    fn read_source(&mut self, path: &Path, source_name: &str, span: Span) -> Option<String> {
        let remaining = self
            .limits
            .maximum_module_source_bytes
            .saturating_sub(self.source_bytes);
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                self.diagnostic(
                    source_name,
                    RuntimeDiagnosticCode::ModuleResolution,
                    span,
                    format!("could not open module {source_name:?}: {error}"),
                    Some("Check the import spelling and file permissions.".to_owned()),
                    Vec::new(),
                );
                return None;
            }
        };
        let read_limit = u64::try_from(remaining)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::with_capacity(remaining.min(64 * 1024));
        if let Err(error) = file.take(read_limit).read_to_end(&mut bytes) {
            self.diagnostic(
                source_name,
                RuntimeDiagnosticCode::ModuleResolution,
                span,
                format!("could not read module {source_name:?}: {error}"),
                Some("Check the module file permissions and encoding.".to_owned()),
                Vec::new(),
            );
            return None;
        }
        if bytes.len() > remaining {
            self.diagnostic(
                source_name,
                RuntimeDiagnosticCode::ModuleLimit,
                span,
                format!(
                    "module graph exceeds the declared aggregate source limit of {} bytes",
                    self.limits.maximum_module_source_bytes
                ),
                Some(
                    "Reduce the imported graph or raise maximum_module_source_bytes deliberately."
                        .to_owned(),
                ),
                Vec::new(),
            );
            return None;
        }
        self.source_bytes = self.source_bytes.saturating_add(bytes.len());
        match String::from_utf8(bytes) {
            Ok(source) => Some(source),
            Err(error) => {
                self.diagnostic(
                    source_name,
                    RuntimeDiagnosticCode::ModuleResolution,
                    span,
                    format!("module {source_name:?} is not valid UTF-8: {error}"),
                    Some("PhonoScript source files must be UTF-8.".to_owned()),
                    Vec::new(),
                );
                None
            }
        }
    }

    fn resolve_import(
        &mut self,
        importer: &Path,
        importer_name: &str,
        path: &str,
        path_span: Span,
    ) -> Option<PathBuf> {
        let requested = Path::new(path);
        if path.is_empty() || requested.is_absolute() {
            self.diagnostic(
                importer_name,
                RuntimeDiagnosticCode::ModuleResolution,
                path_span,
                "module imports must name a nonempty relative .phont path",
                Some(
                    "Use a path such as \"./analysis.phont\" within the declared module root."
                        .to_owned(),
                ),
                Vec::new(),
            );
            return None;
        }
        if !requested
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
        {
            self.diagnostic(
                importer_name,
                RuntimeDiagnosticCode::ModuleResolution,
                path_span,
                format!("module import {path:?} does not use the .{EXTENSION} extension"),
                Some("Only local PhonoScript source modules may be imported.".to_owned()),
                Vec::new(),
            );
            return None;
        }
        let Some(parent) = importer.parent() else {
            self.diagnostic(
                importer_name,
                RuntimeDiagnosticCode::ModuleResolution,
                path_span,
                "the importing module has no parent directory",
                None,
                Vec::new(),
            );
            return None;
        };
        let candidate = parent.join(requested);
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) => {
                self.diagnostic(
                    importer_name,
                    RuntimeDiagnosticCode::ModuleResolution,
                    path_span,
                    format!("could not resolve module import {path:?}: {error}"),
                    Some("Check that the relative .phont file exists beneath the declared module root.".to_owned()),
                    Vec::new(),
                );
                return None;
            }
        };
        if !canonical.starts_with(&self.root) {
            self.diagnostic(
                importer_name,
                RuntimeDiagnosticCode::ModuleResolution,
                path_span,
                format!("module import {path:?} resolves outside the declared module root"),
                Some(
                    "Move the dependency under the module root; symlink escapes are not admitted."
                        .to_owned(),
                ),
                Vec::new(),
            );
            return None;
        }
        if !canonical.is_file() {
            self.diagnostic(
                importer_name,
                RuntimeDiagnosticCode::ModuleResolution,
                path_span,
                format!("module import {path:?} does not resolve to a file"),
                None,
                Vec::new(),
            );
            return None;
        }
        Some(canonical)
    }

    fn visit(&mut self, path: PathBuf, depth: usize, via: Option<ImportFrame>) {
        if depth > self.limits.maximum_module_depth {
            let (source_name, span) = via.as_ref().map_or_else(
                || (self.source_name(&path), empty_source_span()),
                |frame| (frame.source_name.clone(), frame.span),
            );
            self.diagnostic(
                source_name,
                RuntimeDiagnosticCode::ModuleLimit,
                span,
                format!(
                    "module graph exceeds the declared import depth of {}",
                    self.limits.maximum_module_depth
                ),
                Some(
                    "Flatten the dependency graph or raise maximum_module_depth deliberately."
                        .to_owned(),
                ),
                Vec::new(),
            );
            return;
        }

        match self.states.get(&path).copied() {
            Some(ModuleVisitState::Complete) => return,
            Some(ModuleVisitState::Visiting) => {
                let cycle_start = self
                    .module_stack
                    .iter()
                    .position(|candidate| candidate == &path)
                    .unwrap_or(0);
                let mut cycle = self.module_stack[cycle_start..]
                    .iter()
                    .map(|item| self.source_name(item))
                    .collect::<Vec<_>>();
                cycle.push(self.source_name(&path));
                let mut frames = self.import_stack[cycle_start.min(self.import_stack.len())..]
                    .iter()
                    .map(|frame| CallSite {
                        function: "import".to_owned(),
                        source_name: frame.source_name.clone(),
                        span: frame.span,
                    })
                    .collect::<Vec<_>>();
                if let Some(frame) = &via {
                    frames.push(CallSite {
                        function: "import".to_owned(),
                        source_name: frame.source_name.clone(),
                        span: frame.span,
                    });
                }
                let (source_name, span) = via.as_ref().map_or_else(
                    || (self.source_name(&path), empty_source_span()),
                    |frame| (frame.source_name.clone(), frame.span),
                );
                self.diagnostic(
                    source_name,
                    RuntimeDiagnosticCode::ModuleCycle,
                    span,
                    format!("cyclic module dependency: {}", cycle.join(" -> ")),
                    Some("Break the cycle by moving shared immutable definitions into an acyclic module.".to_owned()),
                    frames,
                );
                return;
            }
            None => {}
        }

        if self.states.len() >= self.limits.maximum_modules {
            let (source_name, span) = via.as_ref().map_or_else(
                || (self.source_name(&path), empty_source_span()),
                |frame| (frame.source_name.clone(), frame.span),
            );
            self.diagnostic(
                source_name,
                RuntimeDiagnosticCode::ModuleLimit,
                span,
                format!(
                    "module graph exceeds the declared limit of {} distinct files",
                    self.limits.maximum_modules
                ),
                Some(
                    "Reduce the dependency graph or raise maximum_modules deliberately.".to_owned(),
                ),
                Vec::new(),
            );
            return;
        }

        self.states.insert(path.clone(), ModuleVisitState::Visiting);
        self.module_stack.push(path.clone());
        if let Some(frame) = via.clone() {
            self.import_stack.push(frame);
        }
        let source_name = self.source_name(&path);
        let source_span = via.as_ref().map_or(empty_source_span(), |frame| frame.span);
        let Some(source) = self.read_source(&path, &source_name, source_span) else {
            self.finish_visit(&path, via.is_some());
            return;
        };
        let parsed = frontend::parse(&source);
        self.diagnostics
            .extend(frontend_diagnostics(&source_name, &parsed));
        if !parsed.has_errors() {
            self.diagnostics
                .extend(analysis_diagnostics(&source_name, &parsed.program));
        }
        if path != self.entry {
            for statement in &parsed.program.statements {
                let admitted = match &statement.kind {
                    StatementKind::Import { .. } | StatementKind::Function { .. } => true,
                    StatementKind::Binding {
                        mutable: false,
                        initializer: Some(initializer),
                        ..
                    } => module_initializer_is_declarative(initializer),
                    _ => false,
                };
                if !admitted {
                    self.diagnostic(
                        &source_name,
                        RuntimeDiagnosticCode::ModuleDeclaration,
                        statement.span,
                        "imported modules may contain only imports, functions, and immutable side-effect-free let declarations",
                        Some(
                            "Move project mutation, output, assertions, and file effects into an exported function that the entry module calls explicitly. Module let initializers may use literals, collections, names, indexing, members, and operators, but not calls or assignments."
                                .to_owned(),
                        ),
                        Vec::new(),
                    );
                }
            }
        }

        let export_names = parsed
            .program
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StatementKind::Binding {
                    exported: true,
                    name,
                    ..
                }
                | StatementKind::Function {
                    exported: true,
                    name,
                    ..
                } => Some(name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let imports = parsed
            .program
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StatementKind::Import {
                    path,
                    path_span,
                    bindings,
                    ..
                } => Some((statement.span, path.clone(), *path_span, bindings.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        self.units.insert(
            path.clone(),
            ModuleUnit {
                source_name: source_name.clone(),
                program: parsed.program,
                export_names,
            },
        );

        for (import_span, requested, path_span, bindings) in imports {
            let Some(target) = self.resolve_import(&path, &source_name, &requested, path_span)
            else {
                continue;
            };
            self.edges.insert(
                ModuleEdgeKey {
                    importer: path.clone(),
                    import_byte: import_span.start.byte,
                },
                target.clone(),
            );
            self.visit(
                target.clone(),
                depth.saturating_add(1),
                Some(ImportFrame {
                    source_name: source_name.clone(),
                    span: import_span,
                }),
            );
            if let Some(mut names) = self
                .units
                .get(&target)
                .map(|target_unit| target_unit.export_names.clone())
            {
                let available = names.iter().cloned().collect::<HashSet<_>>();
                for binding in bindings {
                    if !available.contains(&binding.imported) {
                        names.sort();
                        let help = if names.is_empty() {
                            "The target module declares no exports.".to_owned()
                        } else {
                            format!("Available exports: {}.", names.join(", "))
                        };
                        self.diagnostic(
                            &source_name,
                            RuntimeDiagnosticCode::ModuleExport,
                            binding.imported_span,
                            format!(
                                "module {requested:?} does not export {:?}",
                                binding.imported
                            ),
                            Some(help),
                            Vec::new(),
                        );
                    }
                }
            }
        }
        self.finish_visit(&path, via.is_some());
    }

    fn finish_visit(&mut self, path: &Path, had_via: bool) {
        if had_via {
            self.import_stack.pop();
        }
        self.module_stack.pop();
        self.states
            .insert(path.to_path_buf(), ModuleVisitState::Complete);
    }
}

fn empty_source_span() -> Span {
    Span::empty(frontend::SourcePosition::start())
}

fn module_initializer_is_declarative(expression: &Expression) -> bool {
    match &expression.kind {
        ExpressionKind::Literal(_) | ExpressionKind::Variable(_) => true,
        ExpressionKind::List(values) => values.iter().all(module_initializer_is_declarative),
        ExpressionKind::Record(entries) => entries
            .iter()
            .all(|entry| module_initializer_is_declarative(&entry.value)),
        ExpressionKind::Group(value) | ExpressionKind::Unary { operand: value, .. } => {
            module_initializer_is_declarative(value)
        }
        ExpressionKind::Binary { left, right, .. } => {
            module_initializer_is_declarative(left) && module_initializer_is_declarative(right)
        }
        ExpressionKind::Index { collection, index } => {
            module_initializer_is_declarative(collection)
                && module_initializer_is_declarative(index)
        }
        ExpressionKind::Member { object, .. } => module_initializer_is_declarative(object),
        ExpressionKind::Assignment { .. } | ExpressionKind::Call { .. } => false,
    }
}

fn module_entry_diagnostic(
    source_name: impl Into<String>,
    code: RuntimeDiagnosticCode,
    message: impl Into<String>,
    help: Option<String>,
) -> RuntimeDiagnostic {
    RuntimeDiagnostic {
        source_name: source_name.into(),
        code: code.as_str().to_owned(),
        severity: Severity::Error,
        message: message.into(),
        primary: empty_source_span(),
        related: Vec::new(),
        help,
        call_stack: Vec::new(),
    }
}

fn prepare_module_graph(
    entry_path: &Path,
    module_root: &Path,
    limits: RuntimeLimits,
) -> (Option<PreparedModuleGraph>, Vec<RuntimeDiagnostic>) {
    let root = match fs::canonicalize(module_root) {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            return (
                None,
                vec![module_entry_diagnostic(
                    module_root.display().to_string(),
                    RuntimeDiagnosticCode::ModuleResolution,
                    "the declared module root is not a directory",
                    None,
                )],
            );
        }
        Err(error) => {
            return (
                None,
                vec![module_entry_diagnostic(
                    module_root.display().to_string(),
                    RuntimeDiagnosticCode::ModuleResolution,
                    format!("could not resolve the declared module root: {error}"),
                    Some("Choose an existing directory as the module root.".to_owned()),
                )],
            );
        }
    };
    if !entry_path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
    {
        return (
            None,
            vec![module_entry_diagnostic(
                entry_path.display().to_string(),
                RuntimeDiagnosticCode::ModuleResolution,
                format!("entry module must use the .{EXTENSION} extension"),
                None,
            )],
        );
    }
    let entry = match fs::canonicalize(entry_path) {
        Ok(entry) => entry,
        Err(error) => {
            return (
                None,
                vec![module_entry_diagnostic(
                    entry_path.display().to_string(),
                    RuntimeDiagnosticCode::ModuleResolution,
                    format!("could not resolve entry module: {error}"),
                    None,
                )],
            );
        }
    };
    if !entry.starts_with(&root) {
        return (
            None,
            vec![module_entry_diagnostic(
                entry_path.display().to_string(),
                RuntimeDiagnosticCode::ModuleResolution,
                "entry module resolves outside the declared module root",
                Some("Choose a module root that contains the canonical entry file.".to_owned()),
            )],
        );
    }
    if !entry.is_file() {
        return (
            None,
            vec![module_entry_diagnostic(
                entry_path.display().to_string(),
                RuntimeDiagnosticCode::ModuleResolution,
                "entry module does not resolve to a file",
                None,
            )],
        );
    }
    let mut builder = ModuleGraphBuilder::new(root, entry.clone(), limits);
    builder.visit(entry.clone(), 0, None);
    let has_errors = builder
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    let graph = (!has_errors).then_some(PreparedModuleGraph {
        entry,
        units: builder.units,
        edges: builder.edges,
    });
    (graph, builder.diagnostics)
}

/// Execute one script with production limits and transactional document
/// semantics.
pub fn run(source: &str, initial: &ConvalgenDocument) -> RunResult {
    run_named(ANONYMOUS_SOURCE_NAME, source, initial)
}

/// Execute one named source file with production limits and transactional
/// document semantics. The name is retained in every emitted diagnostic and
/// runtime call frame.
pub fn run_named(source_name: &str, source: &str, initial: &ConvalgenDocument) -> RunResult {
    run_with_limits_named(source_name, source, initial, RuntimeLimits::default())
}

/// Parse and statically analyse source without executing it or touching a
/// project. Editors and `phonoscript --check` use this exact pipeline, so live
/// diagnostics cannot drift from the interpreter's admission rules.
pub fn check(source: &str) -> Vec<RuntimeDiagnostic> {
    check_named(ANONYMOUS_SOURCE_NAME, source)
}

/// Parse and statically analyse a named source file without executing it.
pub fn check_named(source_name: &str, source: &str) -> Vec<RuntimeDiagnostic> {
    let parsed = frontend::parse(source);
    let mut diagnostics = frontend_diagnostics(source_name, &parsed);
    if !parsed.has_errors() {
        diagnostics.extend(analysis_diagnostics(source_name, &parsed.program));
        diagnostics.extend(imports_require_module_root(source_name, &parsed.program));
    }
    diagnostics
}

/// Parse, resolve, and statically analyse a local file module graph. Imports
/// are canonicalized relative to each importing file and confined beneath the
/// explicit `module_root`; no user code or project mutation occurs.
pub fn check_file(entry_path: &Path, module_root: &Path) -> Vec<RuntimeDiagnostic> {
    check_file_with_limits(entry_path, module_root, RuntimeLimits::default())
}

/// File-module counterpart to [`check_file`] with explicit deterministic
/// graph limits.
pub fn check_file_with_limits(
    entry_path: &Path,
    module_root: &Path,
    limits: RuntimeLimits,
) -> Vec<RuntimeDiagnostic> {
    let (_, diagnostics) = prepare_module_graph(entry_path, module_root, limits);
    diagnostics
}

fn frontend_diagnostics(
    source_name: &str,
    parsed: &frontend::FrontendOutput,
) -> Vec<RuntimeDiagnostic> {
    parsed
        .diagnostics
        .iter()
        .map(|diagnostic| RuntimeDiagnostic {
            source_name: source_name.to_owned(),
            code: diagnostic.code.as_str().to_owned(),
            severity: diagnostic.severity,
            message: diagnostic.message.clone(),
            primary: diagnostic.primary,
            related: diagnostic.related.clone(),
            help: diagnostic.help.clone(),
            call_stack: Vec::new(),
        })
        .collect()
}

fn analysis_diagnostics(source_name: &str, program: &frontend::Program) -> Vec<RuntimeDiagnostic> {
    phonoscript_analysis::analyze(program)
        .diagnostics
        .into_iter()
        .map(|diagnostic| RuntimeDiagnostic {
            source_name: source_name.to_owned(),
            code: diagnostic.code.as_str().to_owned(),
            severity: diagnostic.severity,
            message: diagnostic.message,
            primary: diagnostic.primary,
            related: diagnostic.related,
            help: diagnostic.help,
            call_stack: Vec::new(),
        })
        .collect()
}

fn imports_require_module_root(
    source_name: &str,
    program: &frontend::Program,
) -> Vec<RuntimeDiagnostic> {
    program
        .statements
        .iter()
        .filter(|statement| matches!(statement.kind, StatementKind::Import { .. }))
        .map(|statement| RuntimeDiagnostic {
            source_name: source_name.to_owned(),
            code: RuntimeDiagnosticCode::ModuleResolution.as_str().to_owned(),
            severity: Severity::Error,
            message: "imports require file execution under an explicit module root".to_owned(),
            primary: statement.span,
            related: Vec::new(),
            help: Some(
                "Use check_file/run_file in the API or pass --module-root to the PhonoScript CLI."
                    .to_owned(),
            ),
            call_stack: Vec::new(),
        })
        .collect()
}

/// Execute with caller-supplied deterministic resource limits.
pub fn run_with_limits(
    source: &str,
    initial: &ConvalgenDocument,
    limits: RuntimeLimits,
) -> RunResult {
    run_with_limits_named(ANONYMOUS_SOURCE_NAME, source, initial, limits)
}

/// Execute named source with caller-supplied deterministic resource limits.
pub fn run_with_limits_named(
    source_name: &str,
    source: &str,
    initial: &ConvalgenDocument,
    limits: RuntimeLimits,
) -> RunResult {
    if let Some(result) = invalid_initial_result(source_name, initial) {
        return result;
    }
    let parsed = frontend::parse(source);
    let mut admission_diagnostics = frontend_diagnostics(source_name, &parsed);
    if !parsed.has_errors() {
        admission_diagnostics.extend(analysis_diagnostics(source_name, &parsed.program));
        admission_diagnostics.extend(imports_require_module_root(source_name, &parsed.program));
    }
    if admission_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return failed_run(initial, admission_diagnostics);
    }

    execute_admitted_program(
        source_name.to_owned(),
        parsed.program,
        None,
        None,
        admission_diagnostics,
        initial,
        limits,
    )
}

/// Execute a local PhonoScript file and its selectively imported modules under
/// an explicit canonical root. The complete graph shares one private project
/// clone and one pending-effect queue; success commits once, while any module
/// admission or runtime failure returns the unchanged initial document.
pub fn run_file(entry_path: &Path, module_root: &Path, initial: &ConvalgenDocument) -> RunResult {
    run_file_with_limits(entry_path, module_root, initial, RuntimeLimits::default())
}

/// File-module counterpart to [`run_file`] with explicit deterministic graph
/// and interpreter limits.
pub fn run_file_with_limits(
    entry_path: &Path,
    module_root: &Path,
    initial: &ConvalgenDocument,
    limits: RuntimeLimits,
) -> RunResult {
    let entry_label = entry_path.display().to_string();
    if let Some(result) = invalid_initial_result(&entry_label, initial) {
        return result;
    }
    let (graph, diagnostics) = prepare_module_graph(entry_path, module_root, limits);
    let Some(graph) = graph else {
        return failed_run(initial, diagnostics);
    };
    let Some(entry_unit) = graph.units.get(&graph.entry).cloned() else {
        return failed_run(
            initial,
            vec![module_entry_diagnostic(
                entry_label,
                RuntimeDiagnosticCode::InternalState,
                "the admitted module graph has no entry unit",
                None,
            )],
        );
    };
    let entry = graph.entry.clone();
    execute_admitted_program(
        entry_unit.source_name,
        entry_unit.program,
        Some(entry),
        Some(graph),
        diagnostics,
        initial,
        limits,
    )
}

fn invalid_initial_result(source_name: &str, initial: &ConvalgenDocument) -> Option<RunResult> {
    initial.validate().err().map(|message| {
        failed_run(
            initial,
            vec![RuntimeDiagnostic {
                source_name: source_name.to_owned(),
                code: RuntimeDiagnosticCode::DomainFormation.as_str().to_owned(),
                severity: Severity::Error,
                message: format!("the initial project is not formed: {message}"),
                primary: empty_source_span(),
                related: Vec::new(),
                help: Some(
                    "Open and repair or migrate the project before executing it.".to_owned(),
                ),
                call_stack: Vec::new(),
            }],
        )
    })
}

fn failed_run(initial: &ConvalgenDocument, diagnostics: Vec<RuntimeDiagnostic>) -> RunResult {
    RunResult {
        document: initial.clone(),
        committed: false,
        value: Value::Null,
        standard_output: Vec::new(),
        diagnostics,
        boundary_conversions: Vec::new(),
        selected_tableau: SelectedTableau::Source,
        statistics: RuntimeStatistics::default(),
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_admitted_program(
    source_name: String,
    program: frontend::Program,
    current_module: Option<PathBuf>,
    module_graph: Option<PreparedModuleGraph>,
    admission_diagnostics: Vec<RuntimeDiagnostic>,
    initial: &ConvalgenDocument,
    limits: RuntimeLimits,
) -> RunResult {
    let builtin_environment = Environment::root();

    let mut runtime = Runtime {
        source_name,
        current_module,
        initial: initial.clone(),
        document: initial.clone(),
        engine: PhonologicalEngine::new(),
        limits,
        statistics: RuntimeStatistics::default(),
        output: Vec::new(),
        output_bytes: 0,
        call_stack: Vec::new(),
        selected_tableau: TableauSlot::Source,
        serial_side: SerialSide::Source,
        effects: Vec::new(),
        warnings: admission_diagnostics,
        boundary_conversions: Vec::new(),
        generation_results: BTreeMap::new(),
        next_generation_handle: 1,
        last_value: Value::Null,
        builtin_environment: builtin_environment.clone(),
        module_graph,
        loaded_modules: HashMap::new(),
        loading_modules: HashSet::new(),
    };
    runtime.install_builtins(&builtin_environment);
    let entry_environment = Environment::child(builtin_environment);

    let execution = runtime.execute_statements(&program.statements, entry_environment, false);
    let execution = execution.and_then(|flow| match flow {
        Flow::Continue(value) => {
            runtime.last_value = value;
            Ok(())
        }
        Flow::Return(_) => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::ReturnOutsideFunction,
            program.span,
            "return may only appear inside a function",
        )),
    });
    let execution = execution.and_then(|()| {
        runtime.document.validate().map_err(|message| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                program.span,
                format!("the completed project is not formed: {message}"),
            )
        })
    });
    let execution = execution.and_then(|()| runtime.commit_effects(program.span));

    match execution {
        Ok(()) => {
            runtime.statistics.queued_file_effects = runtime.effects.len();
            RunResult {
                document: runtime.document,
                committed: true,
                value: runtime.last_value,
                standard_output: runtime.output,
                diagnostics: runtime.warnings,
                boundary_conversions: runtime.boundary_conversions,
                selected_tableau: runtime.selected_tableau.into(),
                statistics: runtime.statistics,
            }
        }
        Err(fault) => {
            let diagnostic = runtime.diagnostic(fault);
            let mut diagnostics = runtime.warnings;
            diagnostics.push(diagnostic);
            RunResult {
                document: runtime.initial,
                committed: false,
                value: Value::Null,
                standard_output: runtime.output,
                diagnostics,
                boundary_conversions: runtime.boundary_conversions,
                selected_tableau: runtime.selected_tableau.into(),
                statistics: runtime.statistics,
            }
        }
    }
}

impl Runtime {
    fn diagnostic(&self, fault: RuntimeFault) -> RuntimeDiagnostic {
        let call_stack = if fault.call_stack.is_empty() {
            self.call_stack.clone()
        } else {
            fault.call_stack
        };
        RuntimeDiagnostic {
            source_name: fault
                .source_name
                .unwrap_or_else(|| self.source_name.clone()),
            code: fault.code.as_str().to_owned(),
            severity: Severity::Error,
            message: fault.message,
            primary: fault.span,
            related: Vec::new(),
            help: fault.help,
            call_stack,
        }
    }

    fn tick(&mut self, span: Span) -> Result<(), RuntimeFault> {
        self.statistics.steps = self.statistics.steps.saturating_add(1);
        if self.statistics.steps > self.limits.maximum_steps {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::StepLimit,
                span,
                format!(
                    "execution exceeded the declared limit of {} steps",
                    self.limits.maximum_steps
                ),
            ));
        }
        Ok(())
    }

    fn execute_statements(
        &mut self,
        statements: &[Statement],
        environment: EnvironmentRef,
        in_function: bool,
    ) -> Result<Flow, RuntimeFault> {
        let mut last = Value::Null;
        for statement in statements {
            match self.execute_statement(statement, environment.clone(), in_function)? {
                Flow::Continue(value) => last = value,
                returned @ Flow::Return(_) => return Ok(returned),
            }
        }
        Ok(Flow::Continue(last))
    }

    fn execute_scoped_block(
        &mut self,
        statements: &[Statement],
        parent: EnvironmentRef,
        in_function: bool,
    ) -> Result<Flow, RuntimeFault> {
        self.execute_statements(statements, Environment::child(parent), in_function)
    }

    fn execute_statement(
        &mut self,
        statement: &Statement,
        environment: EnvironmentRef,
        in_function: bool,
    ) -> Result<Flow, RuntimeFault> {
        self.tick(statement.span)?;
        self.statistics.statements = self.statistics.statements.saturating_add(1);
        match &statement.kind {
            StatementKind::Import { bindings, .. } => {
                let exports = self.load_import(statement)?;
                for imported in bindings {
                    let Some(binding) = exports.get(&imported.imported).cloned() else {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::ModuleExport,
                            imported.imported_span,
                            format!(
                                "the admitted module did not provide export {:?}",
                                imported.imported
                            ),
                        ));
                    };
                    self.declare(
                        &environment,
                        &imported.local,
                        Binding {
                            mutable: false,
                            value: binding.value,
                        },
                        imported.local_span,
                    )?;
                }
                Ok(Flow::Continue(Value::Null))
            }
            StatementKind::Binding {
                mutable,
                name,
                initializer,
                ..
            } => {
                let value = if let Some(initializer) = initializer {
                    self.evaluate_expression(initializer, environment.clone())?
                } else {
                    Value::Null
                };
                self.declare(
                    &environment,
                    name,
                    Binding {
                        mutable: *mutable,
                        value: BindingValue::Data(value.clone()),
                    },
                    statement.span,
                )?;
                Ok(Flow::Continue(value))
            }
            StatementKind::Function {
                name,
                parameters,
                body,
                ..
            } => {
                let function = UserFunction {
                    name: name.clone(),
                    source_name: self.source_name.clone(),
                    parameters: parameters.iter().map(|item| item.name.clone()).collect(),
                    body: body.clone(),
                    closure: Rc::downgrade(&environment),
                };
                self.declare(
                    &environment,
                    name,
                    Binding {
                        mutable: false,
                        value: BindingValue::Function(function),
                    },
                    statement.span,
                )?;
                Ok(Flow::Continue(Value::Null))
            }
            StatementKind::Block(statements) => {
                self.execute_scoped_block(statements, environment, in_function)
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_value = self.evaluate_expression(condition, environment.clone())?;
                if self.boolean(condition_value, condition.span)? {
                    self.execute_scoped_block(then_branch, environment, in_function)
                } else if let Some(branch) = else_branch {
                    self.execute_statement(branch, environment, in_function)
                } else {
                    Ok(Flow::Continue(Value::Null))
                }
            }
            StatementKind::While { condition, body } => {
                let mut last = Value::Null;
                loop {
                    let value = self.evaluate_expression(condition, environment.clone())?;
                    if !self.boolean(value, condition.span)? {
                        break;
                    }
                    self.loop_tick(statement.span)?;
                    match self.execute_scoped_block(body, environment.clone(), in_function)? {
                        Flow::Continue(value) => last = value,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Continue(last))
            }
            StatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => {
                let iterable = self.evaluate_expression(iterable, environment.clone())?;
                let values = match iterable {
                    Value::List(values) => values,
                    Value::Text(text) => text
                        .chars()
                        .map(|character| Value::Text(character.to_string()))
                        .collect(),
                    value => {
                        return Err(self.type_fault(
                            statement.span,
                            "list or text",
                            &value,
                            "for loop collection",
                        ));
                    }
                };
                self.check_collection(values.len(), statement.span)?;
                let mut last = Value::Null;
                for value in values {
                    self.loop_tick(statement.span)?;
                    let loop_environment = Environment::child(environment.clone());
                    self.declare(
                        &loop_environment,
                        binding,
                        Binding {
                            mutable: false,
                            value: BindingValue::Data(value),
                        },
                        statement.span,
                    )?;
                    match self.execute_statements(body, loop_environment, in_function)? {
                        Flow::Continue(value) => last = value,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Continue(last))
            }
            StatementKind::Return(expression) => {
                if !in_function {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::ReturnOutsideFunction,
                        statement.span,
                        "return may only appear inside a function",
                    ));
                }
                let value = if let Some(expression) = expression {
                    self.evaluate_expression(expression, environment)?
                } else {
                    Value::Null
                };
                Ok(Flow::Return(value))
            }
            StatementKind::Expression(expression) => {
                let value = self.evaluate_expression(expression, environment)?;
                self.last_value = value.clone();
                Ok(Flow::Continue(value))
            }
            StatementKind::Command(_) => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::UnsupportedCommand,
                statement.span,
                "legacy line commands are not part of PhonoScript 3",
            )
            .help("Use parsed function calls; for example evaluate() or constraint_add(...).")),
        }
    }

    fn load_import(
        &mut self,
        statement: &Statement,
    ) -> Result<HashMap<String, Binding>, RuntimeFault> {
        let importer = self.current_module.clone().ok_or_else(|| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::ModuleResolution,
                statement.span,
                "imports require file execution under an explicit module root",
            )
        })?;
        let edge = ModuleEdgeKey {
            importer,
            import_byte: statement.span.start.byte,
        };
        let target = self
            .module_graph
            .as_ref()
            .and_then(|graph| graph.edges.get(&edge))
            .cloned()
            .ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    statement.span,
                    "the admitted module graph has no edge for this import",
                )
            })?;
        self.load_module(&target, statement.span)
    }

    fn load_module(
        &mut self,
        path: &Path,
        import_span: Span,
    ) -> Result<HashMap<String, Binding>, RuntimeFault> {
        if let Some(module) = self.loaded_modules.get(path) {
            return Ok(module.exports.clone());
        }
        if !self.loading_modules.insert(path.to_path_buf()) {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::ModuleCycle,
                import_span,
                "module entered the runtime loading stack twice after graph admission",
            ));
        }
        let unit = self
            .module_graph
            .as_ref()
            .and_then(|graph| graph.units.get(path))
            .cloned()
            .ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    import_span,
                    "the admitted module graph has no unit for this import",
                )
            })?;
        let previous_source = std::mem::replace(&mut self.source_name, unit.source_name.clone());
        let previous_module = self.current_module.replace(path.to_path_buf());
        let module_environment = Environment::child(self.builtin_environment.clone());
        let execution =
            self.execute_statements(&unit.program.statements, module_environment.clone(), false);
        let execution = execution.and_then(|flow| match flow {
            Flow::Continue(_) => Ok(()),
            Flow::Return(_) => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::ReturnOutsideFunction,
                unit.program.span,
                "return may only appear inside a function",
            )),
        });
        let exports = execution.and_then(|()| {
            let environment = module_environment.try_borrow().map_err(|_| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    unit.program.span,
                    "the module environment is unavailable while collecting exports",
                )
            })?;
            let mut exports = HashMap::new();
            for name in &unit.export_names {
                let Some(binding) = environment.bindings.get(name).cloned() else {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::InternalState,
                        unit.program.span,
                        format!("declared module export {name:?} has no runtime binding"),
                    ));
                };
                exports.insert(name.clone(), binding);
            }
            Ok(exports)
        });
        self.loading_modules.remove(path);
        self.source_name = previous_source;
        self.current_module = previous_module;
        let exports = exports.map_err(|fault| fault.at_source(unit.source_name.clone()))?;
        self.loaded_modules.insert(
            path.to_path_buf(),
            LoadedModule {
                _environment: module_environment,
                exports: exports.clone(),
            },
        );
        self.statistics.modules_loaded = self.statistics.modules_loaded.saturating_add(1);
        Ok(exports)
    }

    fn loop_tick(&mut self, span: Span) -> Result<(), RuntimeFault> {
        self.statistics.loop_iterations = self.statistics.loop_iterations.saturating_add(1);
        if self.statistics.loop_iterations > self.limits.maximum_loop_iterations {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::LoopLimit,
                span,
                format!(
                    "execution exceeded the declared limit of {} loop iterations",
                    self.limits.maximum_loop_iterations
                ),
            ));
        }
        self.tick(span)
    }

    fn declare(
        &self,
        environment: &EnvironmentRef,
        name: &str,
        binding: Binding,
        span: Span,
    ) -> Result<(), RuntimeFault> {
        let mut environment = environment.try_borrow_mut().map_err(|_| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                "the lexical environment is already mutably borrowed",
            )
        })?;
        if environment.bindings.contains_key(name) {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DuplicateName,
                span,
                format!("{name:?} is already declared in this lexical scope"),
            ));
        }
        environment.bindings.insert(name.to_owned(), binding);
        Ok(())
    }

    fn lookup(
        &self,
        environment: &EnvironmentRef,
        name: &str,
        span: Span,
    ) -> Result<Binding, RuntimeFault> {
        let mut current = Some(environment.clone());
        while let Some(scope) = current {
            let scope = scope.try_borrow().map_err(|_| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    span,
                    "the lexical environment is unavailable during lookup",
                )
            })?;
            if let Some(binding) = scope.bindings.get(name) {
                return Ok(binding.clone());
            }
            current = scope.parent.clone();
        }
        Err(RuntimeFault::new(
            RuntimeDiagnosticCode::UndefinedName,
            span,
            format!("undefined name {name:?}"),
        ))
    }

    fn assign(
        &self,
        environment: &EnvironmentRef,
        name: &str,
        value: Value,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let mut current = Some(environment.clone());
        while let Some(scope) = current {
            let parent = {
                let borrowed = scope.try_borrow().map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InternalState,
                        span,
                        "the lexical environment is unavailable during assignment",
                    )
                })?;
                if let Some(binding) = borrowed.bindings.get(name) {
                    if !binding.mutable {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::ImmutableAssignment,
                            span,
                            format!("cannot assign to immutable binding {name:?}"),
                        ));
                    }
                    if !matches!(binding.value, BindingValue::Data(_)) {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::ImmutableAssignment,
                            span,
                            format!("callable {name:?} cannot be reassigned"),
                        ));
                    }
                    drop(borrowed);
                    let mut borrowed = scope.try_borrow_mut().map_err(|_| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InternalState,
                            span,
                            "the lexical environment cannot be updated",
                        )
                    })?;
                    if let Some(binding) = borrowed.bindings.get_mut(name) {
                        binding.value = BindingValue::Data(value.clone());
                        return Ok(value);
                    }
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::InternalState,
                        span,
                        "the binding disappeared during assignment",
                    ));
                }
                borrowed.parent.clone()
            };
            current = parent;
        }
        Err(RuntimeFault::new(
            RuntimeDiagnosticCode::UndefinedName,
            span,
            format!("cannot assign to undefined name {name:?}"),
        ))
    }
}

fn generation_status_name(status: &CompletenessStatus) -> &'static str {
    match status {
        CompletenessStatus::Complete { .. } => "complete",
        CompletenessStatus::Truncated { .. } => "truncated",
        CompletenessStatus::Refused { .. } => "refused",
    }
}

fn structured_value<T: Serialize>(value: &T, span: Span) -> Result<Value, RuntimeFault> {
    let json = serde_json::to_value(value).map_err(|error| {
        RuntimeFault::new(
            RuntimeDiagnosticCode::InternalState,
            span,
            format!("could not expose structured phonology value: {error}"),
        )
    })?;
    json_to_runtime_value(json, span)
}

fn structured_from_value<T: DeserializeOwned>(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<T, RuntimeFault> {
    let json = runtime_value_to_json(value, coordinate, span)?;
    serde_json::from_value(json).map_err(|error| {
        RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate} does not match the structured phonology schema: {error}"),
        )
    })
}

fn structured_candidate_from_value(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<StructuredCandidate, RuntimeFault> {
    let input = runtime_value_to_json(value, coordinate, span)?;
    let candidate: StructuredCandidate =
        serde_json::from_value(input.clone()).map_err(|error| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("{coordinate} does not match StructuredCandidate: {error}"),
            )
        })?;
    let canonical = serde_json::to_value(&candidate).map_err(|error| {
        RuntimeFault::new(
            RuntimeDiagnosticCode::InternalState,
            span,
            format!("could not canonicalize StructuredCandidate: {error}"),
        )
    })?;
    if let Some(unknown) = first_unknown_json_field(&input, &canonical, coordinate) {
        return Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{unknown} is not a field in the StructuredCandidate schema"),
        ));
    }
    let issues = candidate.validate();
    if let Some(first) = issues.first() {
        let mut fault = RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!(
                "{coordinate}.{} is structurally invalid: {}",
                first.path, first.message
            ),
        );
        if issues.len() > 1 {
            fault = fault.help(format!(
                "{} additional structural issue(s) were withheld until the first issue is repaired.",
                issues.len() - 1
            ));
        }
        return Err(fault);
    }
    Ok(candidate)
}

fn first_unknown_json_field(
    supplied: &serde_json::Value,
    canonical: &serde_json::Value,
    coordinate: &str,
) -> Option<String> {
    match (supplied, canonical) {
        (serde_json::Value::Object(supplied), serde_json::Value::Object(canonical)) => {
            for (field, value) in supplied {
                let next = format!("{coordinate}.{field}");
                let Some(canonical_value) = canonical.get(field) else {
                    return Some(next);
                };
                if let Some(unknown) = first_unknown_json_field(value, canonical_value, &next) {
                    return Some(unknown);
                }
            }
            None
        }
        (serde_json::Value::Array(supplied), serde_json::Value::Array(canonical)) => supplied
            .iter()
            .zip(canonical)
            .enumerate()
            .find_map(|(index, (value, canonical_value))| {
                first_unknown_json_field(value, canonical_value, &format!("{coordinate}[{index}]"))
            }),
        _ => None,
    }
}

fn runtime_value_to_json(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<serde_json::Value, RuntimeFault> {
    match value {
        Value::Null => Ok(serde_json::Value::Null),
        Value::Boolean(value) => Ok(serde_json::Value::Bool(*value)),
        Value::Text(value) => Ok(serde_json::Value::String(value.clone())),
        Value::List(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                runtime_value_to_json(value, &format!("{coordinate}[{index}]"), span)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        Value::Record(values) => values
            .iter()
            .map(|(key, value)| {
                runtime_value_to_json(value, &format!("{coordinate}.{key}"), span)
                    .map(|value| (key.clone(), value))
            })
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        Value::Number(Number::Exact(value)) if value.is_integer() => {
            let integer = value.to_integer();
            let number = integer
                .to_i64()
                .map(serde_json::Number::from)
                .or_else(|| integer.to_u64().map(serde_json::Number::from))
                .ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        format!("{coordinate} is outside the structured integer range"),
                    )
                })?;
            Ok(serde_json::Value::Number(number))
        }
        Value::Number(value) => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!(
                "{coordinate} requires an exact integer in structured phonology data; received {value}"
            ),
        )),
    }
}

fn json_to_runtime_value(value: serde_json::Value, span: Span) -> Result<Value, RuntimeFault> {
    match value {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(value) => Ok(Value::Boolean(value)),
        serde_json::Value::String(value) => Ok(Value::Text(value)),
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| json_to_runtime_value(value, span))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        serde_json::Value::Object(values) => values
            .into_iter()
            .map(|(key, value)| json_to_runtime_value(value, span).map(|value| (key, value)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(Value::Record),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                return Ok(Value::Number(Number::exact(BigInt::from(value))));
            }
            if let Some(value) = value.as_u64() {
                return Ok(Value::Number(Number::exact(BigInt::from(value))));
            }
            Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                "structured phonology unexpectedly serialized a noninteger number",
            ))
        }
    }
}

fn value_record<'a>(
    value: &'a Value,
    coordinate: &str,
    span: Span,
) -> Result<&'a BTreeMap<String, Value>, RuntimeFault> {
    match value {
        Value::Record(value) => Ok(value),
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!("{coordinate} must be a record, not {}", value.type_name()),
        )),
    }
}

fn value_list<'a>(
    value: &'a Value,
    coordinate: &str,
    span: Span,
) -> Result<&'a [Value], RuntimeFault> {
    match value {
        Value::List(value) => Ok(value),
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!("{coordinate} must be a list, not {}", value.type_name()),
        )),
    }
}

fn value_text<'a>(value: &'a Value, coordinate: &str, span: Span) -> Result<&'a str, RuntimeFault> {
    match value {
        Value::Text(value) => Ok(value),
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!("{coordinate} must be text, not {}", value.type_name()),
        )),
    }
}

fn value_u64(value: &Value, coordinate: &str, span: Span) -> Result<u64, RuntimeFault> {
    match value {
        Value::Number(Number::Exact(value)) if value.is_integer() && !value.is_negative() => {
            value.to_integer().to_u64().ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::NumericLimit,
                    span,
                    format!("{coordinate} is outside the nonnegative integer range"),
                )
            })
        }
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!(
                "{coordinate} must be an exact nonnegative integer, not {}",
                value.type_name()
            ),
        )),
    }
}

fn value_usize(value: &Value, coordinate: &str, span: Span) -> Result<usize, RuntimeFault> {
    usize::try_from(value_u64(value, coordinate, span)?).map_err(|_| {
        RuntimeFault::new(
            RuntimeDiagnosticCode::NumericLimit,
            span,
            format!("{coordinate} is outside the platform index range"),
        )
    })
}

fn value_i32(value: &Value, coordinate: &str, span: Span) -> Result<i32, RuntimeFault> {
    match value {
        Value::Number(Number::Exact(value)) if value.is_integer() => {
            value.to_integer().to_i32().ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::NumericLimit,
                    span,
                    format!("{coordinate} is outside the 32-bit feature range"),
                )
            })
        }
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!(
                "{coordinate} must be an exact integer, not {}",
                value.type_name()
            ),
        )),
    }
}

fn required_field<'a>(
    record: &'a BTreeMap<String, Value>,
    field: &str,
    coordinate: &str,
    span: Span,
) -> Result<&'a Value, RuntimeFault> {
    record.get(field).ok_or_else(|| {
        RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate} is missing required field {field:?}"),
        )
    })
}

fn reject_unknown_fields(
    record: &BTreeMap<String, Value>,
    admitted: &[&str],
    coordinate: &str,
    span: Span,
) -> Result<(), RuntimeFault> {
    if let Some(field) = record
        .keys()
        .find(|field| !admitted.contains(&field.as_str()))
    {
        return Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate} has unknown field {field:?}"),
        )
        .help(format!("Admitted fields: {}.", admitted.join(", "))));
    }
    Ok(())
}

fn value_indices<T>(
    value: Option<&Value>,
    coordinate: &str,
    span: Span,
    constructor: impl Fn(u32) -> T,
) -> Result<Vec<T>, RuntimeFault> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value_list(value, coordinate, span)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let value = value_u64(value, &format!("{coordinate}[{index}]"), span)?;
            let value = u32::try_from(value).map_err(|_| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::NumericLimit,
                    span,
                    format!("{coordinate}[{index}] exceeds the structural identifier range"),
                )
            })?;
            Ok(constructor(value))
        })
        .collect()
}

fn parse_feature_value(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<FeatureValue, RuntimeFault> {
    match value {
        Value::Boolean(true) => Ok(FeatureValue::Positive),
        Value::Boolean(false) => Ok(FeatureValue::Negative),
        Value::Null => Ok(FeatureValue::Unspecified),
        Value::Number(_) => value_i32(value, coordinate, span).map(FeatureValue::Integer),
        Value::Text(value) => match value.trim().to_ascii_lowercase().as_str() {
            "+" | "positive" => Ok(FeatureValue::Positive),
            "-" | "negative" => Ok(FeatureValue::Negative),
            "0" | "unspecified" => Ok(FeatureValue::Unspecified),
            _ => Ok(FeatureValue::Symbol(value.clone())),
        },
        Value::Record(record) => {
            reject_unknown_fields(record, &["kind", "value"], coordinate, span)?;
            let kind = value_text(
                required_field(record, "kind", coordinate, span)?,
                &format!("{coordinate}.kind"),
                span,
            )?;
            match kind.trim().to_ascii_lowercase().replace('_', "-").as_str() {
                "positive" => Ok(FeatureValue::Positive),
                "negative" => Ok(FeatureValue::Negative),
                "unspecified" => Ok(FeatureValue::Unspecified),
                "symbol" => Ok(FeatureValue::Symbol(
                    value_text(
                        required_field(record, "value", coordinate, span)?,
                        &format!("{coordinate}.value"),
                        span,
                    )?
                    .to_owned(),
                )),
                "integer" => value_i32(
                    required_field(record, "value", coordinate, span)?,
                    &format!("{coordinate}.value"),
                    span,
                )
                .map(FeatureValue::Integer),
                _ => Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!("{coordinate}.kind has unknown feature value kind {kind:?}"),
                )),
            }
        }
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!(
                "{coordinate} is not a feature value; received {}",
                value.type_name()
            ),
        )),
    }
}

fn parse_segment_template(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<SegmentTemplate, RuntimeFault> {
    if let Value::Text(symbol) = value {
        if symbol.is_empty() {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("{coordinate}.symbol cannot be empty"),
            ));
        }
        return Ok(SegmentTemplate::new(symbol.clone()));
    }
    let record = value_record(value, coordinate, span)?;
    reject_unknown_fields(record, &["symbol", "features"], coordinate, span)?;
    let symbol = value_text(
        required_field(record, "symbol", coordinate, span)?,
        &format!("{coordinate}.symbol"),
        span,
    )?;
    if symbol.is_empty() {
        return Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate}.symbol cannot be empty"),
        ));
    }
    let mut segment = SegmentTemplate::new(symbol);
    if let Some(features) = record.get("features") {
        let features = value_record(features, &format!("{coordinate}.features"), span)?;
        for (name, value) in features {
            if name.trim().is_empty() {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!("{coordinate}.features contains an empty feature name"),
                ));
            }
            segment = segment.with_feature(
                FeatureName(name.clone()),
                parse_feature_value(value, &format!("{coordinate}.features.{name}"), span)?,
            );
        }
    }
    Ok(segment)
}

fn parse_morpheme_kind(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<MorphemeKind, RuntimeFault> {
    let value = value_text(value, coordinate, span)?;
    Ok(
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "root" => MorphemeKind::Root,
            "stem" => MorphemeKind::Stem,
            "prefix" => MorphemeKind::Prefix,
            "suffix" => MorphemeKind::Suffix,
            "infix" => MorphemeKind::Infix,
            "reduplicant" => MorphemeKind::Reduplicant,
            "clitic" => MorphemeKind::Clitic,
            _ => MorphemeKind::UserNamed {
                name: value.to_owned(),
            },
        },
    )
}

fn parse_stress(
    value: Option<&Value>,
    coordinate: &str,
    span: Span,
) -> Result<StressLevel, RuntimeFault> {
    let Some(value) = value else {
        return Ok(StressLevel::Unstressed);
    };
    let value = value_text(value, coordinate, span)?;
    match value.trim().to_ascii_lowercase().as_str() {
        "unstressed" | "none" => Ok(StressLevel::Unstressed),
        "secondary" => Ok(StressLevel::Secondary),
        "primary" => Ok(StressLevel::Primary),
        _ => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate} has unknown stress level {value:?}"),
        )),
    }
}

fn parse_tone_value(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<ToneValue, RuntimeFault> {
    match value {
        Value::Text(value) => Ok(ToneValue::Symbol(value.clone())),
        Value::Number(_) => {
            let value = value_i32(value, coordinate, span)?;
            i8::try_from(value).map(ToneValue::Level).map_err(|_| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!("{coordinate} tone level must fit in a signed byte"),
                )
            })
        }
        Value::List(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let value = value_i32(value, &format!("{coordinate}[{index}]"), span)?;
                i8::try_from(value).map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("{coordinate}[{index}] tone level must fit in a signed byte"),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ToneValue::Contour),
        Value::Record(_) => structured_from_value(value, coordinate, span),
        value => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!("{coordinate} is not a tone value: {}", value.type_name()),
        )),
    }
}

fn parse_tier_value(
    value: &Value,
    coordinate: &str,
    span: Span,
) -> Result<TierValue, RuntimeFault> {
    let Value::Record(record) = value else {
        return parse_tone_value(value, coordinate, span).map(TierValue::Tone);
    };
    let kind = value_text(
        required_field(record, "kind", coordinate, span)?,
        &format!("{coordinate}.kind"),
        span,
    )?;
    match kind.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "tone" => parse_tone_value(
            required_field(record, "value", coordinate, span)?,
            &format!("{coordinate}.value"),
            span,
        )
        .map(TierValue::Tone),
        "feature" => Ok(TierValue::Feature {
            name: FeatureName(
                value_text(
                    required_field(record, "name", coordinate, span)?,
                    &format!("{coordinate}.name"),
                    span,
                )?
                .to_owned(),
            ),
            value: parse_feature_value(
                required_field(record, "value", coordinate, span)?,
                &format!("{coordinate}.value"),
                span,
            )?,
        }),
        "symbol" => Ok(TierValue::Symbol(
            value_text(
                required_field(record, "value", coordinate, span)?,
                &format!("{coordinate}.value"),
                span,
            )?
            .to_owned(),
        )),
        _ => Err(RuntimeFault::new(
            RuntimeDiagnosticCode::DomainFormation,
            span,
            format!("{coordinate}.kind has unknown tier value kind {kind:?}"),
        )),
    }
}

fn apply_form_structure(
    input: &mut UnderlyingForm,
    value: &Value,
    span: Span,
) -> Result<(), RuntimeFault> {
    let structure = value_record(value, "form.structure", span)?;
    reject_unknown_fields(
        structure,
        &["morphemes", "prosody", "tiers"],
        "form.structure",
        span,
    )?;

    if let Some(value) = structure.get("morphemes") {
        let values = value_list(value, "form.structure.morphemes", span)?;
        input.0.morphemes = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let coordinate = format!("form.structure.morphemes[{index}]");
                let record = value_record(value, &coordinate, span)?;
                reject_unknown_fields(record, &["label", "kind", "segments"], &coordinate, span)?;
                Ok(Morpheme {
                    id: MorphemeId(u32::try_from(index).map_err(|_| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::NumericLimit,
                            span,
                            "too many morphemes for the structural identifier range",
                        )
                    })?),
                    label: value_text(
                        required_field(record, "label", &coordinate, span)?,
                        &format!("{coordinate}.label"),
                        span,
                    )?
                    .to_owned(),
                    kind: parse_morpheme_kind(
                        required_field(record, "kind", &coordinate, span)?,
                        &format!("{coordinate}.kind"),
                        span,
                    )?,
                    segments: value_indices(
                        record.get("segments"),
                        &format!("{coordinate}.segments"),
                        span,
                        SegmentId,
                    )?,
                })
            })
            .collect::<Result<Vec<_>, RuntimeFault>>()?;
    }

    if let Some(value) = structure.get("prosody") {
        input.0.prosody = parse_prosody(value, span)?;
    }
    if let Some(value) = structure.get("tiers") {
        input.0.tiers = parse_tiers(value, span)?;
    }
    Ok(())
}

fn parse_prosody(value: &Value, span: Span) -> Result<ProsodicStructure, RuntimeFault> {
    let record = value_record(value, "form.structure.prosody", span)?;
    reject_unknown_fields(
        record,
        &["syllables", "moras", "feet", "words"],
        "form.structure.prosody",
        span,
    )?;
    let syllables = record
        .get("syllables")
        .map(|value| value_list(value, "form.structure.prosody.syllables", span))
        .transpose()?
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let coordinate = format!("form.structure.prosody.syllables[{index}]");
            let item = value_record(value, &coordinate, span)?;
            reject_unknown_fields(
                item,
                &["onset", "nucleus", "coda", "stress"],
                &coordinate,
                span,
            )?;
            Ok(Syllable {
                id: SyllableId(u32::try_from(index).map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        "too many syllables for the structural identifier range",
                    )
                })?),
                onset: value_indices(
                    item.get("onset"),
                    &format!("{coordinate}.onset"),
                    span,
                    SegmentId,
                )?,
                nucleus: value_indices(
                    item.get("nucleus"),
                    &format!("{coordinate}.nucleus"),
                    span,
                    SegmentId,
                )?,
                coda: value_indices(
                    item.get("coda"),
                    &format!("{coordinate}.coda"),
                    span,
                    SegmentId,
                )?,
                stress: parse_stress(item.get("stress"), &format!("{coordinate}.stress"), span)?,
            })
        })
        .collect::<Result<Vec<_>, RuntimeFault>>()?;
    let moras = record
        .get("moras")
        .map(|value| value_list(value, "form.structure.prosody.moras", span))
        .transpose()?
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let coordinate = format!("form.structure.prosody.moras[{index}]");
            let item = value_record(value, &coordinate, span)?;
            reject_unknown_fields(item, &["syllable", "bearers"], &coordinate, span)?;
            Ok(Mora {
                id: MoraId(u32::try_from(index).map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        "too many moras for the structural identifier range",
                    )
                })?),
                syllable: SyllableId(
                    u32::try_from(value_usize(
                        required_field(item, "syllable", &coordinate, span)?,
                        &format!("{coordinate}.syllable"),
                        span,
                    )?)
                    .map_err(|_| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::NumericLimit,
                            span,
                            format!("{coordinate}.syllable exceeds the identifier range"),
                        )
                    })?,
                ),
                bearers: value_indices(
                    item.get("bearers"),
                    &format!("{coordinate}.bearers"),
                    span,
                    SegmentId,
                )?,
            })
        })
        .collect::<Result<Vec<_>, RuntimeFault>>()?;
    let feet = record
        .get("feet")
        .map(|value| value_list(value, "form.structure.prosody.feet", span))
        .transpose()?
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let coordinate = format!("form.structure.prosody.feet[{index}]");
            let item = value_record(value, &coordinate, span)?;
            reject_unknown_fields(item, &["syllables", "head"], &coordinate, span)?;
            let head = match item.get("head") {
                None | Some(Value::Null) => None,
                Some(value) => Some(SyllableId(
                    u32::try_from(value_u64(value, &format!("{coordinate}.head"), span)?).map_err(
                        |_| {
                            RuntimeFault::new(
                                RuntimeDiagnosticCode::NumericLimit,
                                span,
                                format!("{coordinate}.head exceeds the identifier range"),
                            )
                        },
                    )?,
                )),
            };
            Ok(Foot {
                id: FootId(u32::try_from(index).map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        "too many feet for the structural identifier range",
                    )
                })?),
                syllables: value_indices(
                    item.get("syllables"),
                    &format!("{coordinate}.syllables"),
                    span,
                    SyllableId,
                )?,
                head,
            })
        })
        .collect::<Result<Vec<_>, RuntimeFault>>()?;
    let words = record
        .get("words")
        .map(|value| value_list(value, "form.structure.prosody.words", span))
        .transpose()?
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let coordinate = format!("form.structure.prosody.words[{index}]");
            let item = value_record(value, &coordinate, span)?;
            reject_unknown_fields(item, &["syllables", "morphemes"], &coordinate, span)?;
            Ok(ProsodicWord {
                id: ProsodicWordId(u32::try_from(index).map_err(|_| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        "too many prosodic words for the structural identifier range",
                    )
                })?),
                syllables: value_indices(
                    item.get("syllables"),
                    &format!("{coordinate}.syllables"),
                    span,
                    SyllableId,
                )?,
                morphemes: value_indices(
                    item.get("morphemes"),
                    &format!("{coordinate}.morphemes"),
                    span,
                    MorphemeId,
                )?,
            })
        })
        .collect::<Result<Vec<_>, RuntimeFault>>()?;
    Ok(ProsodicStructure {
        syllables,
        moras,
        feet,
        words,
    })
}

fn parse_tiers(value: &Value, span: Span) -> Result<Vec<AutosegmentalTier>, RuntimeFault> {
    value_list(value, "form.structure.tiers", span)?
        .iter()
        .enumerate()
        .map(|(tier_index, value)| {
            let coordinate = format!("form.structure.tiers[{tier_index}]");
            let record = value_record(value, &coordinate, span)?;
            reject_unknown_fields(record, &["name", "nodes", "associations"], &coordinate, span)?;
            let name = value_text(
                required_field(record, "name", &coordinate, span)?,
                &format!("{coordinate}.name"),
                span,
            )?
            .to_owned();
            let nodes = record
                .get("nodes")
                .map(|value| value_list(value, &format!("{coordinate}.nodes"), span))
                .transpose()?
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    Ok(TierNode {
                        id: TierNodeId(u32::try_from(index).map_err(|_| {
                            RuntimeFault::new(
                                RuntimeDiagnosticCode::NumericLimit,
                                span,
                                "too many tier nodes for the structural identifier range",
                            )
                        })?),
                        value: parse_tier_value(
                            value,
                            &format!("{coordinate}.nodes[{index}]"),
                            span,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, RuntimeFault>>()?;
            let associations = record
                .get("associations")
                .map(|value| value_list(value, &format!("{coordinate}.associations"), span))
                .transpose()?
                .unwrap_or(&[])
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let association_coordinate = format!("{coordinate}.associations[{index}]");
                    let association = value_record(value, &association_coordinate, span)?;
                    reject_unknown_fields(
                        association,
                        &["node", "target"],
                        &association_coordinate,
                        span,
                    )?;
                    let node = u32::try_from(value_u64(
                        required_field(association, "node", &association_coordinate, span)?,
                        &format!("{association_coordinate}.node"),
                        span,
                    )?)
                    .map(TierNodeId)
                    .map_err(|_| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::NumericLimit,
                            span,
                            format!("{association_coordinate}.node exceeds the identifier range"),
                        )
                    })?;
                    let target_value = required_field(
                        association,
                        "target",
                        &association_coordinate,
                        span,
                    )?;
                    let target = value_record(
                        target_value,
                        &format!("{association_coordinate}.target"),
                        span,
                    )?;
                    reject_unknown_fields(
                        target,
                        &["kind", "id"],
                        &format!("{association_coordinate}.target"),
                        span,
                    )?;
                    let kind = value_text(
                        required_field(target, "kind", &association_coordinate, span)?,
                        &format!("{association_coordinate}.target.kind"),
                        span,
                    )?;
                    let id = u32::try_from(value_u64(
                        required_field(target, "id", &association_coordinate, span)?,
                        &format!("{association_coordinate}.target.id"),
                        span,
                    )?)
                    .map_err(|_| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::NumericLimit,
                            span,
                            format!("{association_coordinate}.target.id exceeds the identifier range"),
                        )
                    })?;
                    let target = match kind.trim().to_ascii_lowercase().as_str() {
                        "segment" => AssociationTarget::Segment(SegmentId(id)),
                        "syllable" => AssociationTarget::Syllable(SyllableId(id)),
                        "mora" => AssociationTarget::Mora(MoraId(id)),
                        _ => {
                            return Err(RuntimeFault::new(
                                RuntimeDiagnosticCode::DomainFormation,
                                span,
                                format!(
                                    "{association_coordinate}.target.kind has unknown target {kind:?}"
                                ),
                            ));
                        }
                    };
                    Ok(TierAssociation { node, target })
                })
                .collect::<Result<Vec<_>, RuntimeFault>>()?;
            Ok(AutosegmentalTier {
                name,
                nodes,
                associations,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Regression and safety tests

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn successful(script: &str) -> RunResult {
        let result = run(script, &ConvalgenDocument::blank());
        assert!(
            result.succeeded(),
            "script failed: {:?}",
            result.diagnostics
        );
        result
    }

    #[test]
    fn exact_arithmetic_scopes_functions_recursion_and_loops_execute() {
        let script = r#"
fn factorial(n) {
    if n <= 1 { return 1 }
    return n * factorial(n - 1)
}

let whole = 1/3 + 2/3
assert_equal(whole, 1)
var sum = 0
for n in range(1, 6) {
    sum = sum + n
}
assert_equal(sum, 15)
{
    let sum = 99
    assert_equal(sum, 99)
}
assert_equal(sum, 15)
factorial(10)
"#;
        let result = successful(script);
        assert_eq!(
            result.value,
            Value::Number(Number::Exact(BigRational::from_integer(BigInt::from(
                3_628_800
            ))))
        );
        assert!(result.statistics.calls >= 10);
    }

    #[test]
    fn records_and_null_execute_with_exact_nested_values() {
        let result = successful(
            r#"
let analysis = {
    denominator: 3,
    "analysis note": null,
    nested: { value: 1/3 },
}
let reordered = {
    nested: { value: 1/3 },
    denominator: 3,
    "analysis note": null,
}
assert_equal(analysis, reordered)
assert_equal(analysis["analysis note"], null)
analysis["nested"]["value"]
"#,
        );
        assert_eq!(
            result.value,
            Value::Number(Number::Exact(BigRational::new(
                BigInt::from(1),
                BigInt::from(3)
            )))
        );
    }

    #[test]
    fn record_failures_are_structured_and_transactional() {
        let initial = ConvalgenDocument::blank();
        let duplicate = run(
            "project_title(\"rollback\")\nlet r = {a: 1, a: 2}\n",
            &initial,
        );
        assert!(!duplicate.committed);
        assert_eq!(duplicate.document, initial);
        assert_eq!(duplicate.diagnostics[0].code, "PSA1006");

        let missing = run("let r = {a: 1}\nr[\"missing\"]\n", &initial);
        assert!(!missing.committed);
        assert_eq!(missing.diagnostics[0].code, "PSR0403");

        let limited = run_with_limits(
            "let r = {a: 1, b: 2}\n",
            &initial,
            RuntimeLimits {
                maximum_collection_items: 1,
                ..RuntimeLimits::default()
            },
        );
        assert!(!limited.committed);
        assert_eq!(limited.diagnostics[0].code, "PSR0204");
    }

    #[test]
    fn record_member_access_chains_and_reports_the_exact_missing_field() {
        let result =
            successful("let result = {statistics: {retained: 3}}\nresult.statistics.retained\n");
        assert_eq!(result.value, Runtime::integer_value(3));

        let missing = run(
            "let result = {status: \"complete\"}\nresult.reason\n",
            &ConvalgenDocument::blank(),
        );
        assert!(!missing.committed);
        let diagnostic = missing
            .diagnostics
            .iter()
            .find(|item| item.code == "PSR0403")
            .expect("missing member runtime diagnostic");
        assert_eq!(diagnostic.message, "record has no field \"reason\"");
        assert_eq!(diagnostic.primary.start.line, 2);
        assert_eq!(
            diagnostic.primary.end.column - diagnostic.primary.start.column,
            6
        );
    }

    #[test]
    fn structured_form_and_all_declared_finite_gen_operations_use_the_native_model() {
        let result = successful(
            r#"
let input = phonological_form("/bad/", [
    {symbol: "b", features: {voice: true, consonantal: true}},
    {symbol: "a", features: {syllabic: true}},
    {symbol: "d", features: {voice: true, consonantal: true}},
], {
    morphemes: [{label: "root", kind: "root", segments: [0, 1, 2]}],
    prosody: {
        syllables: [{onset: [0], nucleus: [1], coda: [2], stress: "primary"}],
        moras: [{syllable: 0, bearers: [1]}],
        feet: [{syllables: [0], head: 0}],
        words: [{syllables: [0], morphemes: [0]}],
    },
    tiers: [{
        name: "tone",
        nodes: [{kind: "tone", value: 5}],
        associations: [{node: 0, target: {kind: "syllable", id: 0}}],
    }],
})

assert_equal(input.morphemes[0].label, "root")
assert_equal(input.prosody.syllables[0].stress, "primary")
assert_equal(input.tiers[0].associations[0].target.kind, "syllable")

let generated = finite_gen(input, {
    name: "all native operations",
    operations: [
        {id: "identity", operation: {kind: "identity"}, max_applications_per_candidate: 1},
        {id: "delete", operation: {kind: "delete", selector: {kind: "at", positions: [2]}}, max_applications_per_candidate: 1},
        {id: "insert", operation: {kind: "insert", inventory: [{symbol: "ə"}], sites: {kind: "final"}}, max_applications_per_candidate: 1},
        {id: "devoice", operation: {kind: "feature-change", selector: {kind: "at", positions: [2]}, feature: "voice", values: [{kind: "negative"}]}, max_applications_per_candidate: 1},
        {id: "swap", operation: {kind: "metathesis", selector: {kind: "all"}, max_distance: 1}, max_applications_per_candidate: 1},
        {id: "suffix", operation: {kind: "affix", morpheme: {label: "PL", kind: {kind: "suffix"}, segments: [{symbol: "s"}]}, site: {kind: "suffix"}}, max_applications_per_candidate: 1},
        {id: "copy", operation: {kind: "reduplicate", domain: {kind: "whole-form"}, site: "suffix"}, max_applications_per_candidate: 1},
        {id: "syllabify", operation: {kind: "syllabify", specification: {nucleus_selector: {kind: "symbol", symbol: "a"}, max_onset: 1, max_coda: 1, allow_empty_onset: true, allow_empty_coda: true}}, max_applications_per_candidate: 1},
        {id: "stress", operation: {kind: "assign-stress", specification: {primary: "initial", secondary: "none"}}, max_applications_per_candidate: 1},
        {id: "tone", operation: {kind: "assign-tone", specification: {tier_name: "tone-generated", inventory: [{kind: "level", value: 3}], targets: "syllables", pattern: "spread-single"}}, max_applications_per_candidate: 1},
    ],
    domain: {max_derivation_steps: 1, max_segments_per_form: 8},
    resources: {max_candidates: 100, max_operation_expansions: 100, max_variants_per_application: 100},
    support_claim: {kind: "complete-for-declared-domain", statement: "all listed operations through one step"},
    deduplication: "preserve-derivations",
})

assert_equal(generated.status, "complete", generated.reasons)
assert(generated.complete)
assert(generated.statistics.retained_candidates > 10)
assert(len(generated.candidates[0].correspondences) > 0)
let imported = generation_to_tableau(generated, [0])
assert_equal(imported.status, "complete")
imported.imported
"#,
        );
        let Value::Number(Number::Exact(imported)) = result.value else {
            panic!("import count expected: {:?}", result.value);
        };
        assert!(imported.to_integer() > BigInt::from(10));
        assert!(
            result
                .document
                .source
                .candidates
                .iter()
                .all(|candidate| candidate.structured.is_some())
        );
        assert!(result.document.source.candidates.iter().any(|candidate| {
            candidate
                .structured
                .as_ref()
                .is_some_and(|structured| !structured.derivation.is_empty())
        }));
    }

    #[test]
    fn structured_candidate_import_preserves_oo_and_sympathy_graphs() {
        let result = successful(
            r#"
let input = phonological_form("/ab/", ["a", "b"])
let generated = finite_gen(input, {
    name: "identity support",
    operations: [{id: "identity", operation: {kind: "identity"}, max_applications_per_candidate: 1}],
    domain: {max_derivation_steps: 1, max_segments_per_form: 2},
    resources: {max_candidates: 2, max_operation_expansions: 2, max_variants_per_application: 2},
    support_claim: {kind: "complete-for-declared-domain", statement: "identity only"},
    deduplication: "structured-representation",
})
let seed = generated.candidates[0]
let oo_form = {
    id: 2,
    label: "OO base",
    role: {kind: "related-surface", relation: "base of the related word"},
    segments: seed.forms["1"].segments,
    morphemes: seed.forms["1"].morphemes,
    prosody: seed.forms["1"].prosody,
    tiers: seed.forms["1"].tiers,
}
let sympathy_form = {
    id: 3,
    label: "sympathetic candidate",
    role: {kind: "sympathetic"},
    segments: seed.forms["1"].segments,
    morphemes: seed.forms["1"].morphemes,
    prosody: seed.forms["1"].prosody,
    tiers: seed.forms["1"].tiers,
}
let structured = {
    id: 17,
    label: "[ab]",
    underlying_form: 0,
    surface_form: 1,
    forms: {
        "0": seed.forms["0"],
        "1": seed.forms["1"],
        "2": oo_form,
        "3": sympathy_form,
    },
    correspondences: [
        seed.correspondences[0],
        {id: 1, label: "OO", kind: {kind: "output-output"}, source_form: 2, target_form: 1,
         links: [
             {id: 0, source: [{kind: "segment", id: 0}], target: [{kind: "segment", id: 0}]},
             {id: 1, source: [{kind: "segment", id: 1}], target: [{kind: "segment", id: 1}]},
         ]},
        {id: 2, label: "Sympathy", kind: {kind: "sympathy"}, source_form: 3, target_form: 1,
         links: [
             {id: 0, source: [{kind: "segment", id: 0}], target: [{kind: "segment", id: 0}]},
             {id: 1, source: [{kind: "segment", id: 1}], target: [{kind: "segment", id: 1}]},
         ]},
    ],
    derivation: [],
    metadata: {analysis: "OO and Sympathy"},
}
constraints_clear()
candidates_clear()
constraint_add("Faith", 1)
candidate_add_structured("faithful", structured, [0], 3/2, 4)
"#,
        );
        let structured = result.document.source.candidates[0]
            .structured
            .as_ref()
            .expect("structured candidate retained");
        assert_eq!(result.document.source.candidates[0].form, "ab");
        assert_eq!(
            result.document.source.candidates[0]
                .base_mass
                .exact_value()
                .expect("script rational remains exact"),
            &BigRational::new(BigInt::from(3), BigInt::from(2))
        );
        assert!(structured.correspondences.iter().any(|graph| {
            matches!(
                graph.kind,
                crate::phonology::CorrespondenceKind::OutputOutput
            )
        }));
        assert!(
            structured.correspondences.iter().any(|graph| {
                matches!(graph.kind, crate::phonology::CorrespondenceKind::Sympathy)
            })
        );
    }

    #[test]
    fn structured_candidate_import_rejects_unknown_and_invalid_structure() {
        let prefix = r#"
let input = phonological_form("/a/", ["a"])
let generated = finite_gen(input, {
    name: "identity support",
    operations: [{id: "identity", operation: {kind: "identity"}, max_applications_per_candidate: 1}],
    domain: {max_derivation_steps: 1, max_segments_per_form: 1},
    resources: {max_candidates: 2, max_operation_expansions: 2, max_variants_per_application: 2},
    support_claim: {kind: "complete-for-declared-domain", statement: "identity only"},
    deduplication: "structured-representation",
})
let seed = generated.candidates[0]
constraints_clear()
candidates_clear()
constraint_add("Faith", 1)
"#;
        let unknown = run(
            &format!(
                "{prefix}\nlet malformed = {{id: seed.id, label: seed.label, underlying_form: seed.underlying_form, surface_form: seed.surface_form, forms: seed.forms, correspondences: seed.correspondences, derivation: seed.derivation, metadata: seed.metadata, invented: true}}\ncandidate_add_structured(\"bad\", malformed, [0])\n"
            ),
            &ConvalgenDocument::blank(),
        );
        assert!(!unknown.committed);
        assert_eq!(unknown.diagnostics[0].code, "PSR0501");
        assert!(
            unknown.diagnostics[0]
                .message
                .contains("candidate.structured.invented")
        );

        let invalid = run(
            &format!(
                "{prefix}\nlet malformed = {{id: seed.id, label: seed.label, underlying_form: seed.underlying_form, surface_form: 99, forms: seed.forms, correspondences: seed.correspondences, derivation: seed.derivation, metadata: seed.metadata}}\ncandidate_add_structured(\"bad\", malformed, [0])\n"
            ),
            &ConvalgenDocument::blank(),
        );
        assert!(!invalid.committed);
        assert_eq!(invalid.diagnostics[0].code, "PSR0501");
        assert!(
            invalid.diagnostics[0]
                .message
                .contains("candidate.surface-form")
        );
    }

    #[test]
    fn finite_gen_statuses_and_incomplete_import_policy_remain_first_class() {
        let script = r#"
let input = phonological_form("/ab/", ["a", "b"])
let complete = finite_gen(input, {
    name: "identity",
    operations: [{id: "identity", operation: {kind: "identity"}, max_applications_per_candidate: 1}],
    domain: {max_derivation_steps: 1, max_segments_per_form: 4},
    resources: {max_candidates: 4, max_operation_expansions: 4, max_variants_per_application: 4},
    support_claim: {kind: "complete-for-declared-domain", statement: "identity only"},
    deduplication: "structured-representation",
})
let exploratory = finite_gen(input, {
    name: "exploratory",
    operations: [{id: "identity", operation: {kind: "identity"}, max_applications_per_candidate: 1}],
    domain: {max_derivation_steps: 1, max_segments_per_form: 4},
    resources: {max_candidates: 4, max_operation_expansions: 4, max_variants_per_application: 4},
    support_claim: {kind: "exploratory"},
    deduplication: "structured-representation",
})
let refused = finite_gen(input, {
    name: "invalid",
    operations: [],
    domain: {max_derivation_steps: 1, max_segments_per_form: 0},
    resources: {max_candidates: 0, max_operation_expansions: 0, max_variants_per_application: 0},
    support_claim: {kind: "complete-for-declared-domain", statement: "invalid bounds"},
    deduplication: "structured-representation",
})
let bounded = finite_gen(input, {
    name: "candidate limit",
    operations: [{id: "delete", operation: {kind: "delete", selector: {kind: "all"}}, max_applications_per_candidate: 1}],
    domain: {max_derivation_steps: 1, max_segments_per_form: 4},
    resources: {max_candidates: 1, max_operation_expansions: 4, max_variants_per_application: 4},
    support_claim: {kind: "complete-for-declared-domain", statement: "one deletion"},
    deduplication: "structured-representation",
})
assert_equal(complete.status, "complete")
assert_equal(exploratory.status, "truncated")
assert_equal(exploratory.reasons[0].code, "exploratory-support")
assert_equal(refused.status, "refused")
assert(refused.reasons[0].coordinate != "")
assert_equal(bounded.status, "truncated")
assert_equal(bounded.reasons[0].code, "candidate-limit")
generation_to_tableau(bounded, [0], true).imported
"#;
        let result = successful(script);
        assert_eq!(result.value, Runtime::integer_value(1));
        assert!(result.document.source.candidates[0].structured.is_some());

        let refused_import = run(
            &format!(
                "{}\ngeneration_to_tableau(bounded, [0])\n",
                script.replace("generation_to_tableau(bounded, [0], true).imported", "")
            ),
            &ConvalgenDocument::blank(),
        );
        assert!(!refused_import.committed);
        assert!(
            refused_import
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "PSR0502")
        );
    }

    #[test]
    fn a_static_error_prevents_document_mutation() {
        let initial = ConvalgenDocument::blank();
        let result = run(
            r#"
project_title("Must roll back")
let fixed = 1
fixed = 2
"#,
            &initial,
        );
        assert!(!result.committed);
        assert_eq!(result.document, initial);
        assert_eq!(result.diagnostics[0].code, "PSA1004");
    }

    #[test]
    fn deterministic_loop_limit_is_a_structured_error() {
        let initial = ConvalgenDocument::blank();
        let result = run_with_limits(
            "var n = 0\nwhile true { n = n + 1 }\n",
            &initial,
            RuntimeLimits {
                maximum_loop_iterations: 8,
                ..RuntimeLimits::default()
            },
        );
        assert!(!result.committed);
        assert_eq!(result.diagnostics[0].code, "PSR0202");
        assert_eq!(result.document, initial);
    }

    #[test]
    fn named_diagnostics_preserve_frontend_and_analysis_related_spans() {
        let parser_diagnostics = check_named("unclosed.phont", "{\n");
        let parser = parser_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "PSP0106")
            .expect("unclosed block diagnostic");
        assert_eq!(parser.source_name, "unclosed.phont");
        assert_eq!(parser.related.len(), 1);
        assert_eq!(parser.related[0].message, "this block starts here");

        let analysis_diagnostics = check_named("duplicate.phont", "let item = 1\nlet item = 2\n");
        let duplicate = analysis_diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "PSA1002")
            .expect("duplicate binding diagnostic");
        assert_eq!(duplicate.source_name, "duplicate.phont");
        assert_eq!(duplicate.primary.start.line, 2);
        assert_eq!(duplicate.related.len(), 1);
        assert_eq!(duplicate.related[0].span.start.line, 1);
    }

    #[test]
    fn named_runtime_faults_retain_source_aware_call_frames() {
        let result = run_named(
            "nested.phont",
            r#"
fn inner(value) { return value / 0 }
fn outer(value) { return inner(value) }
outer(1)
"#,
            &ConvalgenDocument::blank(),
        );
        assert!(!result.succeeded());
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "PSR0402")
            .expect("division-by-zero diagnostic");
        assert_eq!(diagnostic.source_name, "nested.phont");
        assert_eq!(
            diagnostic
                .call_stack
                .iter()
                .map(|frame| frame.function.as_str())
                .collect::<Vec<_>>(),
            ["outer", "inner"]
        );
        assert!(
            diagnostic
                .call_stack
                .iter()
                .all(|frame| frame.source_name == "nested.phont")
        );
    }

    #[test]
    fn strict_ot_has_no_scalar_harmony_but_weighted_evaluators_remain_exact() {
        let setup = r#"
dataset_clear()
tableau_select("source")
constraints_clear()
candidates_clear()
constraint_add("Faith", 3)
constraint_add("Marked", 1)
candidate_add("faithful", "[faithful]", [0, 2])
candidate_add("repair", "[repair]", [1, 0])
"#;

        let ot_evaluation =
            successful(&format!("{setup}\nproject_evaluator(\"OT\")\nevaluate()\n"));
        let Value::Record(evaluation) = ot_evaluation.value else {
            panic!("evaluation should be a record");
        };
        let Value::List(rows) = &evaluation["rows"] else {
            panic!("evaluation rows should be a list");
        };
        assert!(rows.iter().all(|row| {
            matches!(row, Value::Record(fields) if fields["harmony"] == Value::Null)
        }));

        let ot_harmony = run(
            &format!("{setup}\nproject_evaluator(\"OT\")\nharmony(\"faithful\")\n"),
            &ConvalgenDocument::blank(),
        );
        assert!(!ot_harmony.succeeded());
        assert!(ot_harmony.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "PSR0501" && diagnostic.message.contains("undefined for strict OT")
        }));

        let weighted = successful(&format!(
            "{setup}\nproject_evaluator(\"HG\")\nassert_equal(harmony(\"faithful\"), 2)\nproject_evaluator(\"MaxEnt\")\nharmony(\"faithful\")\n"
        ));
        assert_eq!(weighted.value, Runtime::integer_value(2));
    }

    #[test]
    fn one_script_runs_ot_hg_and_maxent_through_the_shared_engine() {
        let result = successful(
            r#"
dataset_clear()
tableau_select("source")
constraints_clear()
candidates_clear()
constraint_add("Faith", 3)
constraint_add("Marked", 1)
candidate_add("faithful", "[faithful]", [0, 2])
candidate_add("repair", "[repair]", [1, 0])

project_evaluator("OT")
assert_winners(["faithful"])
project_evaluator("HG")
assert_winners(["faithful"])
project_evaluator("MaxEnt")
assert_probability("faithful", 0.731058578630, 0.00000000001)
evaluate()
"#,
        );
        let Value::Record(evaluation) = result.value else {
            panic!("evaluation should be a record");
        };
        assert_eq!(evaluation["evaluator"], Value::Text("MaxEnt".to_owned()));
        assert!(result.statistics.engine_calls >= 4);
        assert!(result.statistics.exact_to_engine_conversions >= 2);
    }

    #[test]
    fn tableau_source_locator_changes_only_the_selected_tableau() {
        let result = successful(
            r#"
tableau_select("source")
tableau_source_locator("Kager 1999, p. 27, tableau 2")
tableau_select("target")
tableau_source_locator("Dissertation, ch. 6, second-order tableau")
tableau_select(0)
tableau_source_locator("Field notebook, p. 14")
"#,
        );
        assert_eq!(
            result.document.source.source_locator,
            "Kager 1999, p. 27, tableau 2"
        );
        assert_eq!(
            result.document.target.source_locator,
            "Dissertation, ch. 6, second-order tableau"
        );
        assert_eq!(
            result.document.dataset[0].source_locator,
            "Field notebook, p. 14"
        );
    }

    #[test]
    fn serial_second_order_and_q_calculus_are_script_native() {
        let result = successful(
            r#"
dataset_clear()
tableau_select("source")
constraints_clear()
candidates_clear()
constraint_add("C1", 2)
constraint_add("C2", 1)
candidate_add("a", "[a]", [0, 1], 1, 1)
candidate_add("b", "[b]", [1, 0])
assert_winners(["a"])

serial_side("source")
serial_start("ab")
serial_clear()
serial_move("ab", "a", "delete b", [0, 0])
serial_move("ab", "ab", "faithful", [1, 0])
serial_move("a", "a", "faithful", [0, 0])
assert(serial_evaluate()["formed"])

tableau_copy("source", "target")
tableau_select("target")
violation_set("a", "C1", 2)
violation_set("b", "C1", 0)
second_query("winner_set")
assert_equal(second_compare()["status"], "DISCREPANCY")

tableau_select("source")
assert_equal(constraint_demotion("a")["status"], "learned")
assert_equal(len(partial_ranking_extensions([], 10)["orders"]), 2)
assert(q_ranking_space()["total_rankings"] > 0)
q_clone("C1")
"#,
        );
        assert!(result.statistics.engine_calls >= 7);
    }

    #[test]
    fn finite_generators_create_forms_without_calling_them_outputs() {
        let result = successful(
            r#"
let deletions = generator_delete("abc")
assert_equal(len(deletions), 3)
let insertions = generator_insert("a", ["i", "u"])
assert_equal(len(insertions), 4)
let substitutions = generator_substitute("ab", "xy")
assert_equal(len(substitutions), 4)
let swaps = generator_swap("abc")
assert_equal(len(swaps), 2)
tableau_select("source")
candidates_from_forms(deletions, [0])
assert_equal(len(winners()), 3)
"#,
        );
        assert!(result.succeeded());
    }

    #[test]
    fn adding_a_constraint_after_the_ledger_exists_is_refused() {
        let result = run(
            r#"
constraints_clear()
candidates_clear()
constraint_add("Faith", 1)
candidate_add("faithful", "[faithful]", [0])
constraint_add("Markedness", 1)
"#,
            &ConvalgenDocument::blank(),
        );
        assert!(!result.committed);
        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "PSR0502" && diagnostic.message.contains("cannot invent marks")
        }));
    }

    #[test]
    fn incomplete_ledger_authoring_preserves_unknown_weights_and_typed_refusals() {
        let result = successful(
            r#"
tableau_select("source")
tableau_evaluator("MaxEnt")
constraints_clear()
candidates_clear()
constraint_add_unweighted("C", "published violation column", 0)
candidate_add("a", "[a]", [0])
candidate_add("b", "[b]", [1])
missing_dependency_add("PE-ADMIT-UNPUBLISHED-WEIGHT", "admission", "evaluator", "constraint[0].weight", "the fitted weight was not published", "supply a source-verified fitted weight", "MaxEnt")
"#,
        );
        assert!(result.document.source.constraints[0].weight.is_none());
        let dependency = &result.document.source.missing_dependencies[0];
        assert_eq!(dependency.code, "PE-ADMIT-UNPUBLISHED-WEIGHT");
        assert_eq!(dependency.stage, DependencyStage::Admission);
        assert_eq!(
            dependency.scope,
            DependencyScope::Evaluator {
                evaluator: EvaluatorKind::MaxEnt
            }
        );
        let refusal = PhonologicalEngine::new()
            .evaluate(&result.document.source, EvaluatorKind::MaxEnt, 1.0)
            .expect_err("the declared unknown must not become a zero weight");
        assert_eq!(refusal.code, dependency.code);
        assert_eq!(refusal.coordinate, dependency.coordinate);
        assert_eq!(refusal.message, dependency.message);
        assert_eq!(refusal.remedy, dependency.remedy);

        let emitted = try_emit(&result.document).expect("typed dependency is persistable");
        let restored = successful(&emitted);
        assert_eq!(restored.document, result.document);
    }

    #[test]
    fn dynamic_invalid_missing_dependency_is_source_located_and_transactional() {
        let initial = ConvalgenDocument::blank();
        let result = run_named(
            "published-ledger.phont",
            "let stage = \"evaluation\"\nmissing_dependency_add(\"MISSING\", stage, \"evaluator\", \"constraint.weight\", \"message\", \"remedy\", \"MaxEnt\")\n",
            &initial,
        );
        assert!(!result.committed);
        assert_eq!(result.document, initial);
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "PSR0501")
            .expect("runtime stage refusal");
        assert_eq!(diagnostic.source_name, "published-ledger.phont");
        assert_eq!(diagnostic.primary.start.line, 2);
        assert!(diagnostic.message.contains("formation or admission"));
    }

    #[test]
    fn queued_save_is_not_written_when_a_later_assertion_fails() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/runtime-tests");
        let base = directory.join(format!("rollback-{}", std::process::id()));
        let destination = base.with_extension("ottab");
        let _ = fs::remove_file(&destination);
        let script = format!(
            "save({:?})\nassert(false, \"stop transaction\")\n",
            base.display().to_string()
        );
        let result = run(&script, &ConvalgenDocument::blank());
        assert!(!result.committed);
        assert!(!destination.exists());
    }

    #[test]
    fn exact_model_setter_preserves_the_script_rational_without_a_boundary() {
        let result = successful("project_temperature(1/10)\n");
        assert!(result.boundary_conversions.is_empty());
        assert_eq!(
            result
                .document
                .temperature
                .exact_value()
                .expect("temperature remains exact"),
            &BigRational::new(BigInt::from(1), BigInt::from(10))
        );
        assert!(
            !result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "PSR0701")
        );
    }

    #[test]
    fn v2_emitter_round_trips_every_document_field() {
        let original = crate::reference_cases::dissertation_second_order();
        let source = emit(&original);
        assert!(source.contains("project_restore_v2"));
        let result = run(&source, &ConvalgenDocument::blank());
        assert!(result.succeeded(), "{:?}", result.diagnostics);
        let mut expected = original;
        expected.normalize();
        assert_eq!(result.document, expected);
        assert_eq!(result.selected_tableau, SelectedTableau::Source);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn arbitrary_utf8_source_returns_without_panicking(source in ".{0,512}") {
            let _ = run_with_limits(
                &source,
                &ConvalgenDocument::blank(),
                RuntimeLimits {
                    maximum_steps: 10_000,
                    maximum_loop_iterations: 256,
                    maximum_collection_items: 2_048,
                    maximum_exact_bytes: 8_192,
                    maximum_output_bytes: 8_192,
                    ..RuntimeLimits::default()
                },
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Serial grammar, Second-Order Tableau, and Q-Calculus

impl Runtime {
    fn builtin_serial(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "serial_side" => {
                self.arity(name, arguments, 1, 1, span)?;
                let side = self.text_argument(name, arguments, 0, span)?;
                self.serial_side = match side.trim().to_ascii_lowercase().as_str() {
                    "source" => SerialSide::Source,
                    "target" => SerialSide::Target,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown serial side {side:?}; expected source or target"),
                        ));
                    }
                };
                Ok(Value::Null)
            }
            "serial_start" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?.to_owned();
                self.selected_serial_mut().start = value;
                Ok(Value::Null)
            }
            "serial_limit" => {
                self.arity(name, arguments, 1, 1, span)?;
                let limit = self.exact_usize(&arguments[0], span, "serial step limit")?;
                if limit == 0 || limit > 1_000_000 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "serial step limit must be between 1 and 1,000,000",
                    ));
                }
                self.selected_serial_mut().maximum_steps = limit;
                Ok(Value::Null)
            }
            "serial_clear" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.selected_serial_mut().moves.clear();
                Ok(Value::Null)
            }
            "serial_move" => {
                self.arity(name, arguments, 4, 4, span)?;
                let from = self.text_argument(name, arguments, 0, span)?.to_owned();
                let to = self.text_argument(name, arguments, 1, span)?.to_owned();
                let operation = self.text_argument(name, arguments, 2, span)?.to_owned();
                let marks = self.list_argument(name, arguments, 3, span)?;
                let width = self.serial_tableau().constraints.len();
                if marks.len() != width {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!(
                            "serial move has {} marks for {width} constraints",
                            marks.len()
                        ),
                    ));
                }
                let violations = marks
                    .iter()
                    .map(|value| self.exact_u16(value, span, "serial violation mark"))
                    .collect::<Result<Vec<_>, _>>()?;
                self.selected_serial_mut().moves.push(SerialMove {
                    from,
                    to,
                    operation,
                    violations,
                });
                Ok(Value::Null)
            }
            "serial_evaluate" => {
                self.arity(name, arguments, 0, 0, span)?;
                let tableau = self.serial_tableau().clone();
                let serial = self.selected_serial().clone();
                let evaluator = self.evaluator_for(&tableau);
                let temperature = self.temperature_for(&tableau);
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self
                    .engine
                    .serial(&tableau, &serial, evaluator, temperature)
                    .map_err(|problem| self.engine_fault(span, problem))?;
                Ok(Self::record([
                    (
                        "path".to_owned(),
                        Value::List(result.path.into_iter().map(Value::Text).collect()),
                    ),
                    (
                        "operations".to_owned(),
                        Value::List(result.operations.into_iter().map(Value::Text).collect()),
                    ),
                    ("stopped".to_owned(), Value::Text(result.stopped)),
                    ("formed".to_owned(), Value::Boolean(result.formed)),
                ]))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented serial builtin {name}"),
            )),
        }
    }

    fn builtin_second_order(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "second_query" => {
                self.arity(name, arguments, 1, 1, span)?;
                let query = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.query = match query
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "winner" | "winners" | "winner_set" => QueryKind::WinnerSet,
                    "surface" | "surface_winners" | "surface_winner_set" | "winning_forms" => {
                        QueryKind::SurfaceWinnerSet
                    }
                    "order" | "complete_order" => QueryKind::CompleteOrder,
                    "probability" | "probability_law" => QueryKind::ProbabilityLaw,
                    "support" | "candidate_support" => QueryKind::CandidateSupport,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown second-order query {query:?}"),
                        ));
                    }
                };
                Ok(Value::Null)
            }
            "second_answer_sort"
            | "second_scope"
            | "second_transformation"
            | "second_transport"
            | "second_layer_transport" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?.to_owned();
                match name {
                    "second_answer_sort" => self.document.second_order.answer_sort = value,
                    "second_scope" => self.document.second_order.scope = value,
                    "second_transformation" => self.document.second_order.transformation = value,
                    "second_transport" => self.document.second_order.transport = value,
                    _ => self.document.second_order.layer_transport = value,
                }
                Ok(Value::Null)
            }
            "second_layout" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.layout = match value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "overlay" => SecondOrderLayout::Overlay,
                    "delta" | "delta_sidecar" | "sidecar" => SecondOrderLayout::DeltaSidecar,
                    "paired" | "expanded" | "expanded_paired" => SecondOrderLayout::ExpandedPaired,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown second-order layout {value:?}"),
                        ));
                    }
                };
                Ok(Value::Null)
            }
            "second_mode" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.comparison_mode = match value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "exact" => ComparisonMode::Exact,
                    "approximate" | "approx" => ComparisonMode::Approximate,
                    "grid" | "grid_based" => ComparisonMode::Grid,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown comparison mode {value:?}"),
                        ));
                    }
                };
                Ok(Value::Null)
            }
            "second_tolerance" | "second_grid_step" => {
                self.arity(name, arguments, 1, 1, span)?;
                let number = self.number_argument(name, arguments, 0, span)?.clone();
                let value = self.number_to_f64(&number, span)?;
                if (name == "second_tolerance" && value < 0.0)
                    || (name == "second_grid_step" && value <= 0.0)
                {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        if name == "second_tolerance" {
                            "second-order tolerance must be nonnegative"
                        } else {
                            "second-order grid step must be strictly positive"
                        },
                    ));
                }
                if name == "second_tolerance" {
                    self.document.second_order.tolerance =
                        self.scalar_from_number(&number, span, "second-order tolerance")?;
                } else {
                    self.document.second_order.grid_step =
                        self.scalar_from_number(&number, span, "second-order grid step")?;
                }
                Ok(Value::Null)
            }
            "second_response_domain" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.response_domain =
                    match value.trim().to_ascii_lowercase().as_str() {
                        "terminal" | "terminal_result" => ResponseDomain::Terminal,
                        "trajectory" | "complete_trajectory" => ResponseDomain::Trajectory,
                        _ => {
                            return Err(RuntimeFault::new(
                                RuntimeDiagnosticCode::DomainFormation,
                                span,
                                format!("unknown response domain {value:?}"),
                            ));
                        }
                    };
                Ok(Value::Null)
            }
            "second_normalizer" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.normalizer_policy = match value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "independent" | "independent_normalizers" => NormalizerPolicy::Independent,
                    "shared" | "shared_declared" | "shared_normalizer" => {
                        NormalizerPolicy::SharedDeclared
                    }
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown normalizer policy {value:?}"),
                        ));
                    }
                };
                Ok(Value::Null)
            }
            "second_layers" => {
                self.arity(name, arguments, 2, 2, span)?;
                self.document.second_order.source_layer =
                    self.text_argument(name, arguments, 0, span)?.to_owned();
                self.document.second_order.target_layer =
                    self.text_argument(name, arguments, 1, span)?.to_owned();
                Ok(Value::Null)
            }
            "second_consumer" => {
                self.arity(name, arguments, 2, 2, span)?;
                let mode = self.text_argument(name, arguments, 0, span)?;
                self.document.second_order.consumer_mode = match mode
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "direct" => ConsumerMode::Direct,
                    "later" | "later_consumer" => ConsumerMode::LaterConsumer,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown consumer mode {mode:?}"),
                        ));
                    }
                };
                self.document.second_order.consumer =
                    self.text_argument(name, arguments, 1, span)?.to_owned();
                Ok(Value::Null)
            }
            "second_compare" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self.engine.compare(&self.document);
                let discrepancies = result
                    .discrepancies
                    .into_iter()
                    .map(|item| {
                        Self::record([
                            ("coordinate".to_owned(), Value::Text(item.coordinate)),
                            ("source".to_owned(), Value::Text(item.source)),
                            ("target".to_owned(), Value::Text(item.target)),
                            ("difference".to_owned(), Value::Text(item.difference)),
                        ])
                    })
                    .collect();
                let refusal = result
                    .refusal
                    .map(|item| {
                        Self::record([
                            ("code".to_owned(), Value::Text(item.code)),
                            (
                                "stage".to_owned(),
                                Value::Text(item.stage.label().to_owned()),
                            ),
                            ("coordinate".to_owned(), Value::Text(item.coordinate)),
                            ("message".to_owned(), Value::Text(item.message)),
                            ("remedy".to_owned(), Value::Text(item.remedy)),
                        ])
                    })
                    .unwrap_or(Value::Null);
                let certificate = result
                    .certificate
                    .map(|item| {
                        Self::record([
                            ("statement".to_owned(), Value::Text(item.statement)),
                            (
                                "evidence".to_owned(),
                                Value::List(item.evidence.into_iter().map(Value::Text).collect()),
                            ),
                        ])
                    })
                    .unwrap_or(Value::Null);
                Ok(Self::record([
                    (
                        "status".to_owned(),
                        Value::Text(result.status.label().to_owned()),
                    ),
                    (
                        "conservative".to_owned(),
                        Value::Boolean(result.status == ComparisonStatus::Preserved),
                    ),
                    (
                        "source_answer".to_owned(),
                        nested_text(result.source_answer),
                    ),
                    (
                        "transported_source_answer".to_owned(),
                        nested_text(result.transported_source_answer),
                    ),
                    (
                        "target_answer".to_owned(),
                        nested_text(result.target_answer),
                    ),
                    ("discrepancies".to_owned(), Value::List(discrepancies)),
                    ("refusal".to_owned(), refusal),
                    ("certificate".to_owned(), certificate),
                    (
                        "source_normalizer".to_owned(),
                        result
                            .source_normalizer
                            .map(Value::Text)
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "target_normalizer".to_owned(),
                        result
                            .target_normalizer
                            .map(Value::Text)
                            .unwrap_or(Value::Null),
                    ),
                ]))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented second-order builtin {name}"),
            )),
        }
    }

    fn builtin_q(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "q_ranking_space" | "typology" => {
                self.arity(name, arguments, 0, 0, span)?;
                let tableaus = self.analysis_dataset(span)?;
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self
                    .engine
                    .q_ranking_space(
                        &tableaus,
                        &self.document.a_priori_rankings,
                        self.document.evaluator,
                        self.document.temperature.to_f64_center().map_err(|error| {
                            RuntimeFault::new(
                                RuntimeDiagnosticCode::DomainBoundary,
                                span,
                                format!("project temperature: {error}"),
                            )
                        })?,
                    )
                    .map_err(|problem| self.engine_fault(span, problem))?;
                let outcomes = result
                    .winner_counts
                    .into_iter()
                    .map(|(answer, count)| {
                        Self::record([
                            (
                                "answer".to_owned(),
                                Value::List(answer.into_iter().map(Value::Text).collect()),
                            ),
                            ("rankings".to_owned(), Self::biguint_value(count)),
                        ])
                    })
                    .collect();
                Ok(Self::record([
                    (
                        "total_rankings".to_owned(),
                        Self::biguint_value(result.total_rankings),
                    ),
                    ("outcomes".to_owned(), Value::List(outcomes)),
                    (
                        "dynamic_states".to_owned(),
                        Self::integer_value(result.dynamic_states),
                    ),
                    (
                        "completion_states".to_owned(),
                        Self::integer_value(result.completion_states),
                    ),
                    (
                        "state_budget".to_owned(),
                        Self::integer_value(result.state_budget),
                    ),
                    (
                        "elapsed_seconds".to_owned(),
                        Self::approximate(
                            result.elapsed.as_secs_f64(),
                            span,
                            "Q-Calculus elapsed time",
                        )?,
                    ),
                ]))
            }
            "q_clone" => {
                self.arity(name, arguments, 1, 1, span)?;
                let tableau = self.selected_tableau(span)?.clone();
                let constraint = self.resolve_constraint(&arguments[0], &tableau, span)?;
                let evaluator = self.evaluator_for(&tableau);
                let temperature = self.temperature_for(&tableau);
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self
                    .engine
                    .q_clone_audit(
                        &tableau,
                        constraint,
                        &self.document.a_priori_rankings,
                        evaluator,
                        temperature,
                    )
                    .map_err(|problem| self.engine_fault(span, problem))?;
                let shifts = result
                    .shifts
                    .into_iter()
                    .map(|shift| {
                        Self::record([
                            (
                                "answer".to_owned(),
                                Value::List(shift.answer.into_iter().map(Value::Text).collect()),
                            ),
                            (
                                "before_numerator".to_owned(),
                                Self::biguint_value(shift.before.numerator().clone()),
                            ),
                            (
                                "before_denominator".to_owned(),
                                Self::biguint_value(shift.before.denominator().clone()),
                            ),
                            (
                                "after_numerator".to_owned(),
                                Self::biguint_value(shift.after.numerator().clone()),
                            ),
                            (
                                "after_denominator".to_owned(),
                                Self::biguint_value(shift.after.denominator().clone()),
                            ),
                        ])
                    })
                    .collect();
                Ok(Self::record([
                    (
                        "support_conservative".to_owned(),
                        Value::Boolean(result.support_conservative),
                    ),
                    (
                        "shares_conservative".to_owned(),
                        Value::Boolean(result.shares_conservative),
                    ),
                    (
                        "before_total_rankings".to_owned(),
                        Self::biguint_value(result.before.total_rankings),
                    ),
                    (
                        "after_total_rankings".to_owned(),
                        Self::biguint_value(result.after.total_rankings),
                    ),
                    (
                        "state_budget".to_owned(),
                        Self::integer_value(result.before.state_budget),
                    ),
                    ("shifts".to_owned(), Value::List(shifts)),
                ]))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented Q-Calculus builtin {name}"),
            )),
        }
    }
}

fn nested_text(rows: Vec<Vec<String>>) -> Value {
    Value::List(
        rows.into_iter()
            .map(|row| Value::List(row.into_iter().map(Value::Text).collect()))
            .collect(),
    )
}

impl Runtime {
    fn builtin_ranking(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "mark_data" | "constraint_demotion" => {
                self.arity(name, arguments, 1, 1, span)?;
                let tableau = self.selected_tableau(span)?.clone();
                let winner = self.resolve_candidate(&arguments[0], &tableau, span)?;
                let evaluator = self.evaluator_for(&tableau);
                let temperature = self.temperature_for(&tableau);
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let data = self
                    .engine
                    .mark_data(&tableau, winner, evaluator, temperature)
                    .map_err(|problem| self.engine_fault(span, problem))?;
                if name == "mark_data" {
                    Ok(self.mark_data_value(&tableau, &data))
                } else {
                    let result = self.engine.constraint_demotion(&data);
                    if let ConstraintDemotionResult::Learned {
                        constraint_strata, ..
                    } = &result
                    {
                        let selected = self.selected_tableau_mut(span)?;
                        for (constraint, stratum) in
                            selected.constraints.iter_mut().zip(constraint_strata)
                        {
                            constraint.stratum = *stratum;
                        }
                    }
                    Ok(self.demotion_value(&tableau, result))
                }
            }
            "partial_ranking_extensions" => {
                self.arity(name, arguments, 2, 2, span)?;
                let tableau = self.selected_tableau(span)?.clone();
                let edges = self.list_argument(name, arguments, 0, span)?;
                let mut dominance = Vec::new();
                for edge in edges {
                    let Value::List(pair) = edge else {
                        return Err(self.type_fault(
                            span,
                            "two-item list",
                            edge,
                            "partial-ranking edge",
                        ));
                    };
                    if pair.len() != 2 {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            "each partial-ranking edge must have exactly two constraints",
                        ));
                    }
                    dominance.push((
                        self.resolve_constraint(&pair[0], &tableau, span)?,
                        self.resolve_constraint(&pair[1], &tableau, span)?,
                    ));
                }
                dominance.sort_unstable();
                dominance.dedup();
                let limit = self.exact_usize(&arguments[1], span, "linear-extension limit")?;
                if limit == 0 || limit > self.limits.maximum_collection_items {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::CollectionLimit,
                        span,
                        format!(
                            "linear-extension limit must be between 1 and {}",
                            self.limits.maximum_collection_items
                        ),
                    ));
                }
                let partial = PartialRanking {
                    constraint_names: tableau
                        .constraints
                        .iter()
                        .map(|constraint| constraint.name.clone())
                        .collect(),
                    dominance,
                };
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self.engine.linear_extensions(&partial, limit);
                Ok(self.linear_extensions_value(&partial, result))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented ranking builtin {name}"),
            )),
        }
    }

    fn mark_data_value(&self, tableau: &Tableau, data: &MarkData) -> Value {
        let constraint_name = |index: &usize| {
            data.constraint_names
                .get(*index)
                .cloned()
                .unwrap_or_else(|| format!("constraint[{index}]"))
        };
        let candidate_name = |index: usize| {
            tableau
                .candidates
                .get(index)
                .map(|candidate| candidate.name.clone())
                .unwrap_or_else(|| format!("candidate[{index}]"))
        };
        let rows = data
            .rows
            .iter()
            .map(|row| {
                Self::record([
                    (
                        "winner_candidate".to_owned(),
                        Value::Text(candidate_name(row.winner_candidate)),
                    ),
                    (
                        "loser_candidate".to_owned(),
                        Value::Text(candidate_name(row.loser_candidate)),
                    ),
                    (
                        "loser_marks".to_owned(),
                        Value::List(
                            row.loser_marks
                                .iter()
                                .map(&constraint_name)
                                .map(Value::Text)
                                .collect(),
                        ),
                    ),
                    (
                        "winner_marks".to_owned(),
                        Value::List(
                            row.winner_marks
                                .iter()
                                .map(&constraint_name)
                                .map(Value::Text)
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect();
        let discarded = data
            .discarded
            .iter()
            .map(|row| {
                Self::record([
                    (
                        "winner_candidate".to_owned(),
                        Value::Text(candidate_name(row.winner_candidate)),
                    ),
                    (
                        "loser_candidate".to_owned(),
                        Value::Text(candidate_name(row.loser_candidate)),
                    ),
                    ("reason".to_owned(), Value::Text(row.reason.clone())),
                ])
            })
            .collect();
        Self::record([
            (
                "constraints".to_owned(),
                Value::List(
                    data.constraint_names
                        .iter()
                        .cloned()
                        .map(Value::Text)
                        .collect(),
                ),
            ),
            ("rows".to_owned(), Value::List(rows)),
            ("discarded".to_owned(), Value::List(discarded)),
        ])
    }

    fn demotion_value(&self, tableau: &Tableau, result: ConstraintDemotionResult) -> Value {
        let names = |indices: Vec<usize>| {
            Value::List(
                indices
                    .into_iter()
                    .map(|index| {
                        tableau
                            .constraints
                            .get(index)
                            .map(|constraint| constraint.name.clone())
                            .unwrap_or_else(|| format!("constraint[{index}]"))
                    })
                    .map(Value::Text)
                    .collect(),
            )
        };
        match result {
            ConstraintDemotionResult::Learned {
                strata,
                trace,
                unresolved_pairs,
                ..
            } => Self::record([
                ("status".to_owned(), Value::Text("learned".to_owned())),
                (
                    "strata".to_owned(),
                    Value::List(strata.into_iter().map(&names).collect()),
                ),
                ("trace_steps".to_owned(), Self::integer_value(trace.len())),
                (
                    "unresolved_pairs".to_owned(),
                    Value::List(
                        unresolved_pairs
                            .into_iter()
                            .map(|(left, right)| names(vec![left, right]))
                            .collect(),
                    ),
                ),
            ]),
            ConstraintDemotionResult::Inconsistent {
                code,
                message,
                trace,
                conflicting_data,
            } => Self::record([
                ("status".to_owned(), Value::Text("inconsistent".to_owned())),
                ("code".to_owned(), Value::Text(code)),
                ("message".to_owned(), Value::Text(message)),
                ("trace_steps".to_owned(), Self::integer_value(trace.len())),
                (
                    "conflicting_data".to_owned(),
                    Value::List(
                        conflicting_data
                            .into_iter()
                            .map(Self::integer_value)
                            .collect(),
                    ),
                ),
            ]),
        }
    }

    fn linear_extensions_value(&self, ranking: &PartialRanking, result: LinearExtensions) -> Value {
        let orders_value = |orders: Vec<Vec<usize>>| {
            Value::List(
                orders
                    .into_iter()
                    .map(|order| {
                        Value::List(
                            order
                                .into_iter()
                                .map(|index| {
                                    ranking
                                        .constraint_names
                                        .get(index)
                                        .cloned()
                                        .unwrap_or_else(|| format!("constraint[{index}]"))
                                })
                                .map(Value::Text)
                                .collect(),
                        )
                    })
                    .collect(),
            )
        };
        match result {
            LinearExtensions::Complete { orders } => Self::record([
                ("status".to_owned(), Value::Text("complete".to_owned())),
                ("orders".to_owned(), orders_value(orders)),
            ]),
            LinearExtensions::Truncated {
                orders,
                limit,
                message,
            } => Self::record([
                ("status".to_owned(), Value::Text("truncated".to_owned())),
                ("orders".to_owned(), orders_value(orders)),
                ("limit".to_owned(), Self::integer_value(limit)),
                ("message".to_owned(), Value::Text(message)),
            ]),
            LinearExtensions::Refused { code, message } => Self::record([
                ("status".to_owned(), Value::Text("refused".to_owned())),
                ("code".to_owned(), Value::Text(code)),
                ("message".to_owned(), Value::Text(message)),
            ]),
        }
    }
}

// ---------------------------------------------------------------------------
// Finite GEN helpers and transactional file effects

impl Runtime {
    fn builtin_generation(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "segments" => {
                self.arity(name, arguments, 1, 1, span)?;
                let form = self.text_argument(name, arguments, 0, span)?;
                let values = form
                    .chars()
                    .map(|segment| Value::Text(segment.to_string()))
                    .collect::<Vec<_>>();
                self.check_collection(values.len(), span)?;
                Ok(Value::List(values))
            }
            "unique" => {
                self.arity(name, arguments, 1, 1, span)?;
                let mut result = Vec::new();
                for value in self.list_argument(name, arguments, 0, span)? {
                    if !result.contains(value) {
                        result.push(value.clone());
                    }
                }
                Ok(Value::List(result))
            }
            "generator_identity" => {
                self.arity(name, arguments, 1, 1, span)?;
                Ok(Value::List(vec![Value::Text(
                    self.text_argument(name, arguments, 0, span)?.to_owned(),
                )]))
            }
            "generator_delete" | "generator_swap" => {
                self.arity(name, arguments, 1, 1, span)?;
                let form = self.text_argument(name, arguments, 0, span)?;
                let segments: Vec<char> = form.chars().collect();
                let mut forms = Vec::new();
                if name == "generator_delete" {
                    for removed in 0..segments.len() {
                        let candidate: String = segments
                            .iter()
                            .enumerate()
                            .filter_map(|(index, segment)| (index != removed).then_some(*segment))
                            .collect();
                        if !forms.contains(&candidate) {
                            forms.push(candidate);
                        }
                    }
                } else {
                    for index in 0..segments.len().saturating_sub(1) {
                        let mut candidate = segments.clone();
                        candidate.swap(index, index + 1);
                        let candidate: String = candidate.into_iter().collect();
                        if candidate != form && !forms.contains(&candidate) {
                            forms.push(candidate);
                        }
                    }
                }
                self.check_collection(forms.len(), span)?;
                Ok(Value::List(forms.into_iter().map(Value::Text).collect()))
            }
            "generator_insert" | "generator_substitute" => {
                self.arity(name, arguments, 2, 2, span)?;
                let form = self.text_argument(name, arguments, 0, span)?;
                let inventory = self.text_inventory(name, arguments, 1, span)?;
                let segments: Vec<char> = form.chars().collect();
                let projected = if name == "generator_insert" {
                    (segments.len() + 1).saturating_mul(inventory.len())
                } else {
                    segments.len().saturating_mul(inventory.len())
                };
                self.check_collection(projected, span)?;
                let mut forms = Vec::new();
                if name == "generator_insert" {
                    for position in 0..=segments.len() {
                        for addition in &inventory {
                            let mut candidate = segments.clone();
                            candidate.insert(position, *addition);
                            let candidate: String = candidate.into_iter().collect();
                            if !forms.contains(&candidate) {
                                forms.push(candidate);
                            }
                        }
                    }
                } else {
                    for position in 0..segments.len() {
                        for replacement in &inventory {
                            if segments[position] == *replacement {
                                continue;
                            }
                            let mut candidate = segments.clone();
                            candidate[position] = *replacement;
                            let candidate: String = candidate.into_iter().collect();
                            if !forms.contains(&candidate) {
                                forms.push(candidate);
                            }
                        }
                    }
                }
                self.check_collection(forms.len(), span)?;
                Ok(Value::List(forms.into_iter().map(Value::Text).collect()))
            }
            "candidates_from_forms" => {
                self.arity(name, arguments, 2, 2, span)?;
                let forms = self
                    .list_argument(name, arguments, 0, span)?
                    .iter()
                    .map(|value| match value {
                        Value::Text(form) => Ok(form.clone()),
                        value => Err(self.type_fault(span, "text", value, "candidate form")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                self.check_collection(forms.len(), span)?;
                let width = self.selected_tableau(span)?.constraints.len();
                let violation_matrix = self.violation_matrix_argument(
                    &arguments[1],
                    forms.len(),
                    width,
                    span,
                    "candidates_from_forms violation matrix",
                )?;
                let mut identities = HashSet::new();
                let mut candidates = Vec::new();
                for (index, (form, violations)) in
                    forms.into_iter().zip(violation_matrix).enumerate()
                {
                    let base = if form.is_empty() {
                        "∅".to_owned()
                    } else {
                        form.clone()
                    };
                    let mut identity = base.clone();
                    let mut suffix = 2_usize;
                    while !identities.insert(identity.clone()) {
                        identity = format!("{base}#{suffix}");
                        suffix += 1;
                    }
                    candidates.push(Candidate {
                        id: format!("generated-candidate-{}", index + 1),
                        name: identity,
                        form,
                        violations,
                        base_mass: NumericScalar::integer(1),
                        notes: format!("generated candidate {}", index + 1),
                        observed_frequency: NumericScalar::integer(0),
                        structured: None,
                    });
                }
                self.selected_tableau_mut(span)?.candidates = candidates;
                Ok(Value::Null)
            }
            "phonological_form" => {
                self.arity(name, arguments, 2, 3, span)?;
                let label = self.text_argument(name, arguments, 0, span)?.to_owned();
                if label.trim().is_empty() {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        "phonological_form requires a nonempty label",
                    ));
                }
                let segment_values = self.list_argument(name, arguments, 1, span)?;
                self.check_collection(segment_values.len(), span)?;
                let segments = segment_values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        parse_segment_template(value, &format!("form.segments[{index}]"), span)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let mut input = UnderlyingForm::from_segments(label, segments);
                if let Some(structure) = arguments.get(2) {
                    apply_form_structure(&mut input, structure, span)?;
                }
                input = UnderlyingForm::try_new(input.0).map_err(|error| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("phonological form is not formed: {error}"),
                    )
                })?;
                structured_value(&input, span)
            }
            "finite_gen" => {
                self.arity(name, arguments, 2, 2, span)?;
                let input: UnderlyingForm =
                    structured_from_value(&arguments[0], "finite_gen input", span)?;
                let input = UnderlyingForm::try_new(input.0).map_err(|error| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("finite_gen input is not formed: {error}"),
                    )
                })?;
                let specification: GeneratorSpec =
                    structured_from_value(&arguments[1], "finite_gen specification", span)?;
                self.admit_generator_resources(&specification, span)?;
                let result = FiniteGenerator::generate(&input, &specification);
                self.check_collection(result.candidates.len(), span)?;
                let handle = self.next_generation_handle;
                self.next_generation_handle = handle.checked_add(1).ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::NumericLimit,
                        span,
                        "finite generation handle space is exhausted",
                    )
                })?;
                let value = self.generation_result_value(handle, &result, span)?;
                self.generation_results.insert(handle, result);
                Ok(value)
            }
            "generation_to_tableau" => {
                self.arity(name, arguments, 2, 3, span)?;
                let allow_incomplete = arguments
                    .get(2)
                    .map(|value| match value {
                        Value::Boolean(value) => Ok(*value),
                        value => Err(self.type_fault(
                            span,
                            "boolean",
                            value,
                            "generation_to_tableau incomplete-support policy",
                        )),
                    })
                    .transpose()?
                    .unwrap_or(false);
                self.generation_to_tableau(&arguments[0], &arguments[1], allow_incomplete, span)
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented generation builtin {name}"),
            )),
        }
    }

    fn text_inventory(
        &self,
        name: &str,
        arguments: &[Value],
        index: usize,
        span: Span,
    ) -> Result<Vec<char>, RuntimeFault> {
        let mut inventory = Vec::new();
        match arguments.get(index) {
            Some(Value::Text(value)) => inventory.extend(value.chars()),
            Some(Value::List(values)) => {
                for value in values {
                    let Value::Text(value) = value else {
                        return Err(self.type_fault(span, "text", value, "segment inventory item"));
                    };
                    let mut characters = value.chars();
                    let Some(character) = characters.next() else {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            "segment inventory item cannot be empty",
                        ));
                    };
                    if characters.next().is_some() {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!(
                                "segment inventory item {value:?} contains more than one Unicode scalar"
                            ),
                        ));
                    }
                    inventory.push(character);
                }
            }
            Some(value) => {
                return Err(self.type_fault(span, "text or list", value, "segment inventory"));
            }
            None => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::Arity,
                    span,
                    format!("{name} is missing argument {}", index + 1),
                ));
            }
        }
        inventory.sort_unstable();
        inventory.dedup();
        self.check_collection(inventory.len(), span)?;
        Ok(inventory)
    }

    fn admit_generator_resources(
        &self,
        specification: &GeneratorSpec,
        span: Span,
    ) -> Result<(), RuntimeFault> {
        self.check_collection(specification.operations.len(), span)?;
        let collection_limit = self.limits.maximum_collection_items;
        for (coordinate, value) in [
            (
                "generator.resources.max-candidates",
                specification.resources.max_candidates,
            ),
            (
                "generator.resources.max-variants-per-application",
                specification.resources.max_variants_per_application,
            ),
            (
                "generator.domain.max-segments-per-form",
                specification.domain.max_segments_per_form,
            ),
        ] {
            if value > collection_limit {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::CollectionLimit,
                    span,
                    format!(
                        "{coordinate} is {value}, above the runtime collection limit of {collection_limit}"
                    ),
                ));
            }
        }
        if specification.resources.max_operation_expansions
            > usize::try_from(self.limits.maximum_steps).unwrap_or(usize::MAX)
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::StepLimit,
                span,
                format!(
                    "generator.resources.max-operation-expansions is {}, above the runtime step limit of {}",
                    specification.resources.max_operation_expansions, self.limits.maximum_steps
                ),
            ));
        }
        if specification.domain.max_derivation_steps
            > usize::try_from(self.limits.maximum_loop_iterations).unwrap_or(usize::MAX)
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::LoopLimit,
                span,
                format!(
                    "generator.domain.max-derivation-steps is {}, above the runtime loop limit of {}",
                    specification.domain.max_derivation_steps, self.limits.maximum_loop_iterations
                ),
            ));
        }
        Ok(())
    }

    fn generation_result_value(
        &self,
        handle: u64,
        result: &GenerationResult,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let candidates = result
            .candidates
            .iter()
            .map(|candidate| structured_value(candidate, span))
            .collect::<Result<Vec<_>, _>>()?;
        let statistics = structured_value(&result.statistics, span)?;
        let (status, complete, claim, reasons) = match &result.status {
            CompletenessStatus::Complete { claim } => {
                ("complete", true, structured_value(claim, span)?, Vec::new())
            }
            CompletenessStatus::Truncated { reasons } => (
                "truncated",
                false,
                Value::Null,
                reasons
                    .iter()
                    .map(|reason| structured_value(reason, span))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            CompletenessStatus::Refused { reasons } => (
                "refused",
                false,
                Value::Null,
                reasons
                    .iter()
                    .map(|reason| structured_value(reason, span))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        };
        Ok(Self::record([
            (
                "generation_handle".to_owned(),
                Value::Number(Number::exact(BigInt::from(handle))),
            ),
            ("status".to_owned(), Value::Text(status.to_owned())),
            ("complete".to_owned(), Value::Boolean(complete)),
            ("claim".to_owned(), claim),
            ("reasons".to_owned(), Value::List(reasons)),
            ("statistics".to_owned(), statistics),
            ("candidates".to_owned(), Value::List(candidates)),
        ]))
    }

    fn violation_matrix_argument(
        &self,
        value: &Value,
        row_count: usize,
        width: usize,
        span: Span,
        coordinate: &str,
    ) -> Result<Vec<Vec<u16>>, RuntimeFault> {
        let Value::List(items) = value else {
            return Err(self.type_fault(span, "list", value, coordinate));
        };
        let parse_row = |row: &[Value], row_index: usize| -> Result<Vec<u16>, RuntimeFault> {
            if row.len() != width {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!(
                        "{coordinate} row {} has {} marks for {width} constraints",
                        row_index + 1,
                        row.len()
                    ),
                ));
            }
            row.iter()
                .map(|mark| self.exact_u16(mark, span, "violation mark"))
                .collect()
        };
        if items.iter().all(|item| matches!(item, Value::Number(_))) {
            let row = parse_row(items, 0)?;
            return Ok(vec![row; row_count]);
        }
        if items.len() != row_count {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!(
                    "{coordinate} has {} rows for {row_count} candidates",
                    items.len()
                ),
            ));
        }
        items
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let Value::List(row) = row else {
                    return Err(self.type_fault(span, "list", row, coordinate));
                };
                parse_row(row, index)
            })
            .collect()
    }

    fn generation_to_tableau(
        &mut self,
        value: &Value,
        violations: &Value,
        allow_incomplete: bool,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let record = value_record(value, "generation result", span)?;
        let handle = value_u64(
            required_field(record, "generation_handle", "generation result", span)?,
            "generation result.generation_handle",
            span,
        )?;
        let result = self
            .generation_results
            .get(&handle)
            .cloned()
            .ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!("generation handle {handle} does not belong to this script execution"),
                )
            })?;
        let status = generation_status_name(&result.status);
        if !result.status.is_complete() && !allow_incomplete {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!(
                    "generation status is {status}; pass true as the third argument only to import the explicitly bounded retained support"
                ),
            ));
        }
        if result.candidates.is_empty() {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!("the {status} generation retained no candidates to import"),
            ));
        }
        self.check_collection(result.candidates.len(), span)?;
        let width = self.selected_tableau(span)?.constraints.len();
        let violation_matrix = self.violation_matrix_argument(
            violations,
            result.candidates.len(),
            width,
            span,
            "generation_to_tableau violation matrix",
        )?;
        let mut names = HashSet::new();
        let mut candidates = Vec::with_capacity(result.candidates.len());
        for (index, (structured, violations)) in result
            .candidates
            .into_iter()
            .zip(violation_matrix)
            .enumerate()
        {
            let issues = structured.validate();
            if !issues.is_empty() {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!(
                        "generated candidate {} is not formed: {}",
                        index + 1,
                        issues
                            .iter()
                            .map(|issue| format!("{}: {}", issue.path, issue.message))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            }
            let base = if structured.label.trim().is_empty() {
                structured.surface_string()
            } else {
                structured.label.clone()
            };
            let base = if base.is_empty() {
                "∅".to_owned()
            } else {
                base
            };
            let mut candidate_name = base.clone();
            let mut suffix = 2_usize;
            while !names.insert(candidate_name.clone()) {
                candidate_name = format!("{base}#{suffix}");
                suffix += 1;
            }
            candidates.push(Candidate {
                id: format!("structured-candidate-{}", index + 1),
                name: candidate_name,
                form: structured.surface_string(),
                violations,
                base_mass: NumericScalar::integer(1),
                notes: format!(
                    "finite GEN {status}; {} declared derivation step(s)",
                    structured.derivation.len()
                ),
                observed_frequency: NumericScalar::integer(0),
                structured: Some(structured),
            });
        }
        let imported = candidates.len();
        self.selected_tableau_mut(span)?.candidates = candidates;
        Ok(Self::record([
            ("status".to_owned(), Value::Text(status.to_owned())),
            (
                "bounded_support".to_owned(),
                Value::Boolean(!result.status.is_complete()),
            ),
            ("imported".to_owned(), Self::integer_value(imported)),
        ]))
    }

    fn builtin_file(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "save" => {
                self.arity(name, arguments, 1, 1, span)?;
                let path = PathBuf::from(self.text_argument(name, arguments, 0, span)?);
                let destination = document::ensure_extension(&path);
                self.queue_effect(
                    PendingEffect::Save {
                        path,
                        document: Box::new(self.document.clone()),
                    },
                    &destination,
                    span,
                )?;
                Ok(Value::Text(destination.display().to_string()))
            }
            "export_tableau" => {
                self.arity(name, arguments, 2, 3, span)?;
                let path = PathBuf::from(self.text_argument(name, arguments, 0, span)?);
                let format =
                    self.export_format(self.text_argument(name, arguments, 1, span)?, span)?;
                let second_order = if arguments.len() == 3 {
                    self.boolean_argument(name, arguments, 2, span)?
                } else {
                    false
                };
                let svg = export::tableau_svg(&self.document, second_order).map_err(|message| {
                    RuntimeFault::new(RuntimeDiagnosticCode::FileEffect, span, message)
                })?;
                let destination = path.with_extension(format.extension());
                self.queue_effect(
                    PendingEffect::Export {
                        path,
                        format,
                        scale: self.document.presentation.export_scale,
                        svg,
                    },
                    &destination,
                    span,
                )?;
                Ok(Value::Text(destination.display().to_string()))
            }
            "export_plot" => {
                self.arity(name, arguments, 2, 2, span)?;
                let path = PathBuf::from(self.text_argument(name, arguments, 0, span)?);
                let format =
                    self.export_format(self.text_argument(name, arguments, 1, span)?, span)?;
                let svg = export::plot_svg(&self.document).map_err(|message| {
                    RuntimeFault::new(RuntimeDiagnosticCode::FileEffect, span, message)
                })?;
                let destination = path.with_extension(format.extension());
                self.queue_effect(
                    PendingEffect::Export {
                        path,
                        format,
                        scale: self.document.presentation.export_scale,
                        svg,
                    },
                    &destination,
                    span,
                )?;
                Ok(Value::Text(destination.display().to_string()))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented file builtin {name}"),
            )),
        }
    }

    fn export_format(&self, value: &str, span: Span) -> Result<ExportFormat, RuntimeFault> {
        match value.trim().to_ascii_lowercase().as_str() {
            "svg" => Ok(ExportFormat::Svg),
            "png" => Ok(ExportFormat::Png),
            "pdf" => Ok(ExportFormat::Pdf),
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("unknown export format {value:?}; expected svg, png, or pdf"),
            )),
        }
    }

    fn queue_effect(
        &mut self,
        effect: PendingEffect,
        destination: &Path,
        span: Span,
    ) -> Result<(), RuntimeFault> {
        let duplicate = self.effects.iter().any(|existing| match existing {
            PendingEffect::Save { path, .. } => document::ensure_extension(path) == destination,
            PendingEffect::Export { path, format, .. } => {
                path.with_extension(format.extension()) == destination
            }
        });
        if duplicate {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::FileEffect,
                span,
                format!(
                    "more than one file effect targets {}",
                    destination.display()
                ),
            ));
        }
        self.effects.push(effect);
        Ok(())
    }

    fn commit_effects(&mut self, span: Span) -> Result<(), RuntimeFault> {
        self.statistics.queued_file_effects = self.effects.len();
        if self.effects.is_empty() {
            return Ok(());
        }
        struct Staged {
            stage: PathBuf,
            destination: PathBuf,
            backup: Option<PathBuf>,
        }
        let mut staged = Vec::<Staged>::new();
        for (index, effect) in self.effects.iter().enumerate() {
            let (destination, parent) = match effect {
                PendingEffect::Save { path, .. } => {
                    let destination = document::ensure_extension(path);
                    let parent = destination
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_owned();
                    (destination, parent)
                }
                PendingEffect::Export { path, format, .. } => {
                    let destination = path.with_extension(format.extension());
                    let parent = destination
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_owned();
                    (destination, parent)
                }
            };
            fs::create_dir_all(&parent).map_err(|error| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::FileEffect,
                    span,
                    format!("could not create {}: {error}", parent.display()),
                )
            })?;
            let stem = destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("analysis");
            let mut counter = 0_usize;
            let stage_base = loop {
                let candidate = parent.join(format!(
                    ".phonoscript-{}-{index}-{counter}-{stem}",
                    std::process::id()
                ));
                let candidate_file = match effect {
                    PendingEffect::Save { .. } => document::ensure_extension(&candidate),
                    PendingEffect::Export { format, .. } => {
                        candidate.with_extension(format.extension())
                    }
                };
                if !candidate_file.exists() {
                    break candidate;
                }
                counter = counter.saturating_add(1);
            };
            let stage = match effect {
                PendingEffect::Save {
                    document: snapshot, ..
                } => document::save(&stage_base, snapshot),
                PendingEffect::Export {
                    format, scale, svg, ..
                } => export::write_with_scale(svg, &stage_base, *format, *scale),
            }
            .map_err(|message| {
                for item in &staged {
                    let _ = fs::remove_file(&item.stage);
                }
                RuntimeFault::new(RuntimeDiagnosticCode::FileEffect, span, message)
            })?;
            staged.push(Staged {
                stage,
                destination,
                backup: None,
            });
        }

        for (index, item) in staged.iter_mut().enumerate() {
            if !item.destination.exists() {
                continue;
            }
            let parent = item.destination.parent().unwrap_or_else(|| Path::new("."));
            let name = item
                .destination
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("analysis");
            let backup = parent.join(format!(
                ".phonoscript-{}-{index}-backup-{name}",
                std::process::id()
            ));
            if backup.exists() {
                for staged_item in &staged {
                    let _ = fs::remove_file(&staged_item.stage);
                }
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::FileEffect,
                    span,
                    format!(
                        "transaction backup path already exists: {}",
                        backup.display()
                    ),
                ));
            }
            fs::rename(&item.destination, &backup).map_err(|error| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::FileEffect,
                    span,
                    format!(
                        "could not stage existing {}: {error}",
                        item.destination.display()
                    ),
                )
            })?;
            item.backup = Some(backup);
        }

        for (committed, index) in (0..staged.len()).enumerate() {
            if let Err(error) = fs::rename(&staged[index].stage, &staged[index].destination) {
                for item in staged.iter().take(committed) {
                    let _ = fs::remove_file(&item.destination);
                }
                for item in &staged {
                    if let Some(backup) = &item.backup {
                        let _ = fs::rename(backup, &item.destination);
                    }
                    let _ = fs::remove_file(&item.stage);
                }
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::FileEffect,
                    span,
                    format!(
                        "could not commit {}: {error}",
                        staged[index].destination.display()
                    ),
                ));
            }
        }
        for item in &staged {
            if let Some(backup) = &item.backup {
                let _ = fs::remove_file(backup);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// First-order evaluation and learning

impl Runtime {
    fn evaluate_selected(
        &mut self,
        span: Span,
    ) -> Result<(Tableau, EvaluatorKind, TableauEvaluation), RuntimeFault> {
        let tableau = self.selected_tableau(span)?.clone();
        let evaluator = self.evaluator_for(&tableau);
        let temperature = self.temperature_for(&tableau);
        self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
        let result = self
            .engine
            .evaluate(&tableau, evaluator, temperature)
            .map_err(|problem| self.engine_fault(span, problem))?;
        Ok((tableau, evaluator, result))
    }

    fn evaluation_value(
        &self,
        tableau: &Tableau,
        evaluator: EvaluatorKind,
        evaluation: &TableauEvaluation,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let winners = evaluation
            .winner_indices
            .iter()
            .filter_map(|index| tableau.candidates.get(*index))
            .map(|candidate| Value::Text(candidate.name.clone()))
            .collect::<Vec<_>>();
        let winning_forms = evaluation
            .winner_indices
            .iter()
            .filter_map(|index| tableau.candidates.get(*index))
            .map(|candidate| Value::Text(candidate.form.clone()))
            .collect::<Vec<_>>();
        let rows = evaluation
            .rows
            .iter()
            .map(|row| {
                let candidate = tableau.candidates.get(row.candidate).ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InternalState,
                        span,
                        "engine returned a candidate index outside the tableau",
                    )
                })?;
                let probability = match row.probability {
                    Some(value) => Self::approximate(value, span, "candidate probability")?,
                    None => Value::Null,
                };
                let harmony = match (evaluator, &row.exact_harmony) {
                    // Strict OT compares violation profiles lexicographically;
                    // it has no scalar harmony coordinate.
                    (EvaluatorKind::Ot, _) => Value::Null,
                    (_, Some(value)) => Value::Number(Number::Exact(value.clone())),
                    (_, None) => Self::approximate(row.harmony, span, "candidate harmony")?,
                };
                let fatal = row
                    .fatal_constraint
                    .and_then(|index| tableau.constraints.get(index))
                    .map(|constraint| Value::Text(constraint.name.clone()))
                    .unwrap_or(Value::Null);
                Ok(Self::record([
                    ("candidate".to_owned(), Value::Text(candidate.name.clone())),
                    ("form".to_owned(), Value::Text(candidate.form.clone())),
                    ("harmony".to_owned(), harmony),
                    ("probability".to_owned(), probability),
                    ("winner".to_owned(), Value::Boolean(row.winner)),
                    ("fatal_constraint".to_owned(), fatal),
                ]))
            })
            .collect::<Result<Vec<_>, RuntimeFault>>()?;
        Ok(Self::record([
            (
                "evaluator".to_owned(),
                Value::Text(evaluator.short_label().to_owned()),
            ),
            ("winners".to_owned(), Value::List(winners)),
            ("winning_forms".to_owned(), Value::List(winning_forms)),
            (
                "tie_unresolved".to_owned(),
                Value::Boolean(evaluation.tie_unresolved),
            ),
            ("rows".to_owned(), Value::List(rows)),
        ]))
    }

    fn builtin_evaluation(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let arity = match name {
            "evaluate" | "winners" | "winning_forms" => (0, 0),
            "harmony" | "probability" => (1, 1),
            "assert_winners" | "assert_winning_forms" => (1, 2),
            "assert_probability" => (3, 4),
            _ => (0, 0),
        };
        self.arity(name, arguments, arity.0, arity.1, span)?;
        let (tableau, evaluator, evaluation) = self.evaluate_selected(span)?;
        match name {
            "evaluate" => self.evaluation_value(&tableau, evaluator, &evaluation, span),
            "winners" => Ok(Value::List(
                evaluation
                    .winner_indices
                    .iter()
                    .filter_map(|index| tableau.candidates.get(*index))
                    .map(|candidate| Value::Text(candidate.name.clone()))
                    .collect(),
            )),
            "winning_forms" => Ok(Value::List(
                evaluation
                    .winner_indices
                    .iter()
                    .filter_map(|index| tableau.candidates.get(*index))
                    .map(|candidate| Value::Text(candidate.form.clone()))
                    .collect(),
            )),
            "harmony" | "probability" => {
                let candidate = self.resolve_candidate(&arguments[0], &tableau, span)?;
                let row = evaluation
                    .rows
                    .iter()
                    .find(|row| row.candidate == candidate)
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InternalState,
                            span,
                            "engine omitted the requested candidate",
                        )
                    })?;
                if name == "harmony" {
                    if evaluator == EvaluatorKind::Ot {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            "scalar harmony is undefined for strict OT; inspect the ordered violation profile or winning relation instead",
                        ));
                    }
                    match &row.exact_harmony {
                        Some(value) => Ok(Value::Number(Number::Exact(value.clone()))),
                        None => Self::approximate(row.harmony, span, "candidate harmony"),
                    }
                } else {
                    row.probability
                        .ok_or_else(|| {
                            RuntimeFault::new(
                                RuntimeDiagnosticCode::DomainFormation,
                                span,
                                "candidate probability is defined only for MaxEnt evaluation",
                            )
                        })
                        .and_then(|value| Self::approximate(value, span, "candidate probability"))
                }
            }
            "assert_winners" | "assert_winning_forms" => {
                let expected = self
                    .list_argument(name, arguments, 0, span)?
                    .iter()
                    .map(|value| match value {
                        Value::Text(value) => Ok(value.clone()),
                        value => Err(self.type_fault(span, "text", value, "expected winner list")),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let actual = if name == "assert_winners" {
                    evaluation
                        .winner_indices
                        .iter()
                        .filter_map(|index| tableau.candidates.get(*index))
                        .map(|candidate| candidate.name.clone())
                        .collect::<Vec<_>>()
                } else {
                    evaluation
                        .winner_indices
                        .iter()
                        .filter_map(|index| tableau.candidates.get(*index))
                        .map(|candidate| candidate.form.clone())
                        .collect::<Vec<_>>()
                };
                let mut expected_sorted = expected;
                let mut actual_sorted = actual;
                expected_sorted.sort();
                actual_sorted.sort();
                if expected_sorted == actual_sorted {
                    Ok(Value::Boolean(true))
                } else {
                    let message = arguments.get(1).map(Value::render).unwrap_or_else(|| {
                        format!("expected {expected_sorted:?}, calculated {actual_sorted:?}")
                    });
                    Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::AssertionFailed,
                        span,
                        message,
                    ))
                }
            }
            "assert_probability" => {
                let candidate = self.resolve_candidate(&arguments[0], &tableau, span)?;
                let expected_number = self.number_argument(name, arguments, 1, span)?.clone();
                let tolerance_number = self.number_argument(name, arguments, 2, span)?.clone();
                let expected =
                    self.engine_f64(&expected_number, span, "assert_probability expected value")?;
                let tolerance =
                    self.engine_f64(&tolerance_number, span, "assert_probability tolerance")?;
                if tolerance < 0.0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "probability tolerance must be nonnegative",
                    ));
                }
                let actual = evaluation
                    .rows
                    .iter()
                    .find(|row| row.candidate == candidate)
                    .and_then(|row| row.probability)
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            "assert_probability requires a MaxEnt tableau",
                        )
                    })?;
                if (actual - expected).abs() <= tolerance {
                    Ok(Value::Boolean(true))
                } else {
                    let message = arguments.get(3).map(Value::render).unwrap_or_else(|| {
                        format!("calculated probability {actual} differs from {expected} by more than {tolerance}")
                    });
                    Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::AssertionFailed,
                        span,
                        message,
                    ))
                }
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented evaluation builtin {name}"),
            )),
        }
    }

    fn analysis_dataset(&self, span: Span) -> Result<Vec<Tableau>, RuntimeFault> {
        if self.document.dataset.is_empty() {
            Ok(vec![self.selected_tableau(span)?.clone()])
        } else {
            Ok(self.document.dataset.clone())
        }
    }

    fn builtin_learning(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let tableaus = self.analysis_dataset(span)?;
        match name {
            "maxent_learn" => {
                self.arity(name, arguments, 0, 1, span)?;
                let iterations = if arguments.is_empty() {
                    10_000
                } else {
                    self.exact_usize(&arguments[0], span, "maximum learning iterations")?
                };
                if iterations == 0 || iterations > 10_000_000 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "maximum learning iterations must be between 1 and 10,000,000",
                    ));
                }
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self
                    .engine
                    .learn_maxent(
                        &tableaus,
                        self.document.temperature.to_f64_center().map_err(|error| {
                            RuntimeFault::new(
                                RuntimeDiagnosticCode::DomainBoundary,
                                span,
                                format!("project temperature: {error}"),
                            )
                        })?,
                        iterations,
                    )
                    .map_err(|problem| self.engine_fault(span, problem))?;
                self.apply_constraint_weights(&tableaus[0], &result.weights);
                let weights = result
                    .weights
                    .iter()
                    .map(|value| Self::approximate(*value, span, "learned weight"))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::record([
                    ("weights".to_owned(), Value::List(weights)),
                    (
                        "iterations".to_owned(),
                        Self::integer_value(result.iterations),
                    ),
                    ("converged".to_owned(), Value::Boolean(result.converged)),
                    (
                        "negative_log_likelihood".to_owned(),
                        Self::approximate(
                            result.negative_log_likelihood,
                            span,
                            "negative log likelihood",
                        )?,
                    ),
                    (
                        "maximum_gradient".to_owned(),
                        Self::approximate(result.maximum_gradient, span, "maximum gradient")?,
                    ),
                ]))
            }
            "infer_ranking" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let result = self
                    .engine
                    .infer_ranking(&tableaus, &self.document.a_priori_rankings)
                    .map_err(|problem| self.engine_fault(span, problem))?;
                self.apply_constraint_order(&tableaus[0], &result.order);
                let order = result
                    .order
                    .iter()
                    .filter_map(|index| tableaus[0].constraints.get(*index))
                    .map(|constraint| Value::Text(constraint.name.clone()))
                    .collect();
                Ok(Self::record([
                    ("order".to_owned(), Value::List(order)),
                    (
                        "explored_states".to_owned(),
                        Self::integer_value(result.explored_states),
                    ),
                ]))
            }
            "harmonic_bounds" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let bounds = self
                    .engine
                    .harmonic_bounds(&tableaus)
                    .map_err(|problem| self.engine_fault(span, problem))?;
                Ok(Value::List(
                    bounds
                        .into_iter()
                        .map(|bound| {
                            Self::record([
                                ("input".to_owned(), Value::Text(bound.input)),
                                ("observed_candidate".to_owned(), Value::Text(bound.observed)),
                                (
                                    "bounding_candidate".to_owned(),
                                    Value::Text(bound.bounding_rival),
                                ),
                            ])
                        })
                        .collect(),
                ))
            }
            "unnecessary_constraints" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.statistics.engine_calls = self.statistics.engine_calls.saturating_add(1);
                let indices = self
                    .engine
                    .unnecessary_constraints(&tableaus, &self.document.a_priori_rankings)
                    .map_err(|problem| self.engine_fault(span, problem))?;
                Ok(Value::List(
                    indices
                        .into_iter()
                        .filter_map(|index| tableaus[0].constraints.get(index))
                        .map(|constraint| Value::Text(constraint.name.clone()))
                        .collect(),
                ))
            }
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("unimplemented learning builtin {name}"),
            )),
        }
    }

    fn apply_constraint_weights(&mut self, register: &Tableau, weights: &[f64]) {
        let names: Vec<&str> = register
            .constraints
            .iter()
            .map(|constraint| constraint.name.as_str())
            .collect();
        for tableau in std::iter::once(&mut self.document.source)
            .chain(std::iter::once(&mut self.document.target))
            .chain(self.document.dataset.iter_mut())
        {
            if tableau
                .constraints
                .iter()
                .map(|constraint| constraint.name.as_str())
                .eq(names.iter().copied())
            {
                for (constraint, weight) in tableau.constraints.iter_mut().zip(weights) {
                    let boundary =
                        ApproximationBoundary::new(ApproximationMethod::NumericalOptimization)
                            .and_then(|boundary| boundary.with_source("PhonoScript MaxEnt learner"))
                            .and_then(|boundary| {
                                boundary.with_note(
                                    "fitted by numerical maximum-likelihood optimization",
                                )
                            })
                            .expect("static approximation metadata is valid");
                    constraint.weight = NumericScalar::approximate(*weight, boundary).ok();
                }
            }
        }
    }

    fn apply_constraint_order(&mut self, register: &Tableau, order: &[usize]) {
        let names: Vec<String> = register
            .constraints
            .iter()
            .map(|constraint| constraint.name.clone())
            .collect();
        let strata: HashMap<&str, usize> = order
            .iter()
            .enumerate()
            .filter_map(|(stratum, index)| names.get(*index).map(|name| (name.as_str(), stratum)))
            .collect();
        for tableau in std::iter::once(&mut self.document.source)
            .chain(std::iter::once(&mut self.document.target))
            .chain(self.document.dataset.iter_mut())
        {
            if tableau
                .constraints
                .iter()
                .map(|constraint| constraint.name.as_str())
                .eq(names.iter().map(String::as_str))
            {
                for constraint in &mut tableau.constraints {
                    if let Some(stratum) = strata.get(constraint.name.as_str()) {
                        constraint.stratum = *stratum;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Builtin registration and argument decoding

const BUILTINS: &[&str] = &[
    "print",
    "assert",
    "assert_equal",
    "assert_approx",
    "len",
    "range",
    "type_of",
    "to_text",
    "project_restore_v2",
    "project_title",
    "project_author",
    "project_description",
    "project_keyword",
    "project_evaluator",
    "project_temperature",
    "dataset_clear",
    "tableau_new",
    "tableau_select",
    "tableau_copy",
    "tableau_name",
    "tableau_input",
    "tableau_notes",
    "tableau_source_locator",
    "tableau_evaluator",
    "tableau_temperature",
    "tableau_ties",
    "constraints_clear",
    "constraint_add",
    "constraint_add_unweighted",
    "missing_dependency_add",
    "constraint_remove",
    "constraint_move",
    "constraint_rank",
    "constraint_tie",
    "constraint_weight",
    "constraint_definition",
    "constraint_enabled",
    "constraint_prior",
    "candidates_clear",
    "candidate_add",
    "candidate_add_structured",
    "candidate_remove",
    "candidate_move",
    "candidate_name",
    "candidate_form",
    "candidate_mass",
    "candidate_observed",
    "candidate_notes",
    "violation_set",
    "violation_get",
    "evaluate",
    "winners",
    "winning_forms",
    "harmony",
    "probability",
    "assert_winners",
    "assert_winning_forms",
    "assert_probability",
    "maxent_learn",
    "infer_ranking",
    "harmonic_bounds",
    "unnecessary_constraints",
    "serial_side",
    "serial_start",
    "serial_limit",
    "serial_clear",
    "serial_move",
    "serial_evaluate",
    "second_query",
    "second_answer_sort",
    "second_scope",
    "second_transformation",
    "second_transport",
    "second_layout",
    "second_mode",
    "second_tolerance",
    "second_grid_step",
    "second_response_domain",
    "second_normalizer",
    "second_layers",
    "second_layer_transport",
    "second_consumer",
    "second_compare",
    "q_ranking_space",
    "q_clone",
    "typology",
    "generator_identity",
    "generator_delete",
    "generator_insert",
    "generator_substitute",
    "generator_swap",
    "segments",
    "unique",
    "candidates_from_forms",
    "phonological_form",
    "finite_gen",
    "generation_to_tableau",
    "mark_data",
    "constraint_demotion",
    "partial_ranking_extensions",
    "save",
    "export_tableau",
    "export_plot",
];

impl Runtime {
    fn install_builtins(&self, environment: &EnvironmentRef) {
        if let Ok(mut environment) = environment.try_borrow_mut() {
            for name in BUILTINS {
                environment.bindings.insert(
                    (*name).to_owned(),
                    Binding {
                        mutable: false,
                        value: BindingValue::Builtin(name),
                    },
                );
            }
        }
    }

    fn arity(
        &self,
        name: &str,
        arguments: &[Value],
        minimum: usize,
        maximum: usize,
        span: Span,
    ) -> Result<(), RuntimeFault> {
        if (minimum..=maximum).contains(&arguments.len()) {
            Ok(())
        } else {
            let expected = if minimum == maximum {
                minimum.to_string()
            } else {
                format!("{minimum} through {maximum}")
            };
            Err(RuntimeFault::new(
                RuntimeDiagnosticCode::Arity,
                span,
                format!(
                    "{name} expects {expected} arguments but received {}",
                    arguments.len()
                ),
            ))
        }
    }

    fn text_argument<'a>(
        &self,
        name: &str,
        arguments: &'a [Value],
        index: usize,
        span: Span,
    ) -> Result<&'a str, RuntimeFault> {
        match arguments.get(index) {
            Some(Value::Text(value)) => Ok(value),
            Some(value) => Err(self.type_fault(
                span,
                "text",
                value,
                &format!("argument {} to {name}", index + 1),
            )),
            None => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::Arity,
                span,
                format!("{name} is missing argument {}", index + 1),
            )),
        }
    }

    fn boolean_argument(
        &self,
        name: &str,
        arguments: &[Value],
        index: usize,
        span: Span,
    ) -> Result<bool, RuntimeFault> {
        match arguments.get(index) {
            Some(Value::Boolean(value)) => Ok(*value),
            Some(value) => Err(self.type_fault(
                span,
                "boolean",
                value,
                &format!("argument {} to {name}", index + 1),
            )),
            None => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::Arity,
                span,
                format!("{name} is missing argument {}", index + 1),
            )),
        }
    }

    fn number_argument<'a>(
        &self,
        name: &str,
        arguments: &'a [Value],
        index: usize,
        span: Span,
    ) -> Result<&'a Number, RuntimeFault> {
        match arguments.get(index) {
            Some(Value::Number(value)) => Ok(value),
            Some(value) => Err(self.type_fault(
                span,
                "number",
                value,
                &format!("argument {} to {name}", index + 1),
            )),
            None => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::Arity,
                span,
                format!("{name} is missing argument {}", index + 1),
            )),
        }
    }

    fn list_argument<'a>(
        &self,
        name: &str,
        arguments: &'a [Value],
        index: usize,
        span: Span,
    ) -> Result<&'a [Value], RuntimeFault> {
        match arguments.get(index) {
            Some(Value::List(value)) => Ok(value),
            Some(value) => Err(self.type_fault(
                span,
                "list",
                value,
                &format!("argument {} to {name}", index + 1),
            )),
            None => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::Arity,
                span,
                format!("{name} is missing argument {}", index + 1),
            )),
        }
    }

    fn engine_f64(
        &mut self,
        number: &Number,
        span: Span,
        coordinate: &str,
    ) -> Result<f64, RuntimeFault> {
        let value = self.number_to_f64(number, span).map_err(|mut problem| {
            problem.message = format!("{coordinate}: {}", problem.message);
            problem
        })?;
        if let Number::Exact(exact_value) = number {
            self.statistics.exact_to_engine_conversions = self
                .statistics
                .exact_to_engine_conversions
                .saturating_add(1);
            self.boundary_conversions.push(BoundaryConversion {
                coordinate: coordinate.to_owned(),
                exact_value: exact_value.clone(),
                engine_value: value,
                span,
            });
            self.warnings.push(RuntimeDiagnostic {
                source_name: self.source_name.clone(),
                code: RuntimeDiagnosticCode::ApproximateBoundary
                    .as_str()
                    .to_owned(),
                severity: Severity::Warning,
                message: format!(
                    "exact script value {} crossed into the current f64 engine model at {coordinate}; the engine received binary approximation {value:.17}. This calculation is approximate and cannot license literal-exact theorem status",
                    Number::Exact(exact_value.clone())
                ),
                primary: span,
                related: Vec::new(),
                help: Some(
                    "The exact rational and supplied f64 are retained in RunResult.boundary_conversions for audit and future exact-model lowering."
                        .to_owned(),
                ),
                call_stack: self.call_stack.clone(),
            });
        }
        Ok(value)
    }

    /// Preserve a script number's declared exactness when it enters the
    /// persisted semantic model. This is not an engine boundary.
    fn scalar_from_number(
        &self,
        number: &Number,
        span: Span,
        coordinate: &str,
    ) -> Result<NumericScalar, RuntimeFault> {
        match number {
            Number::Exact(value) => Ok(NumericScalar::exact(value.clone())),
            Number::Approximate(value) => {
                let boundary = ApproximationBoundary::binary_f64()
                    .with_source("PhonoScript approximate number")
                    .and_then(|boundary| boundary.with_note(format!("persisted at {coordinate}")))
                    .map_err(|error| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainBoundary,
                            span,
                            format!("{coordinate}: {error}"),
                        )
                    })?;
                NumericScalar::approximate(*value, boundary).map_err(|error| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        format!("{coordinate}: {error}"),
                    )
                })
            }
        }
    }

    fn approximate(value: f64, span: Span, coordinate: &str) -> Result<Value, RuntimeFault> {
        Number::finite_approximate(value)
            .map(Value::Number)
            .ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainBoundary,
                    span,
                    format!("{coordinate} is nonfinite"),
                )
            })
    }

    fn integer_value(value: usize) -> Value {
        Value::Number(Number::exact(BigInt::from(value)))
    }

    fn biguint_value(value: BigUint) -> Value {
        Value::Number(Number::Exact(BigRational::from_integer(BigInt::from(
            value,
        ))))
    }

    fn record(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Record(entries.into_iter().collect())
    }

    fn engine_fault(&self, span: Span, problem: EngineError) -> RuntimeFault {
        RuntimeFault::new(
            RuntimeDiagnosticCode::EngineRefusal,
            span,
            format!(
                "{} [{}:{}] {}",
                problem.code, problem.stage, problem.coordinate, problem.message
            ),
        )
        .help(problem.remedy)
    }

    fn selected_tableau(&self, span: Span) -> Result<&Tableau, RuntimeFault> {
        match self.selected_tableau {
            TableauSlot::Source => Ok(&self.document.source),
            TableauSlot::Target => Ok(&self.document.target),
            TableauSlot::Dataset(index) => self.document.dataset.get(index).ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    span,
                    format!("selected tableau index {index} no longer exists"),
                )
            }),
        }
    }

    fn selected_tableau_mut(&mut self, span: Span) -> Result<&mut Tableau, RuntimeFault> {
        match self.selected_tableau {
            TableauSlot::Source => Ok(&mut self.document.source),
            TableauSlot::Target => Ok(&mut self.document.target),
            TableauSlot::Dataset(index) => self.document.dataset.get_mut(index).ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    span,
                    format!("selected tableau index {index} no longer exists"),
                )
            }),
        }
    }

    fn selected_serial(&self) -> &SerialSettings {
        match self.serial_side {
            SerialSide::Source => &self.document.serial,
            SerialSide::Target => &self.document.target_serial,
        }
    }

    fn selected_serial_mut(&mut self) -> &mut SerialSettings {
        match self.serial_side {
            SerialSide::Source => &mut self.document.serial,
            SerialSide::Target => &mut self.document.target_serial,
        }
    }

    fn serial_tableau(&self) -> &Tableau {
        match self.serial_side {
            SerialSide::Source => &self.document.source,
            SerialSide::Target => &self.document.target,
        }
    }

    fn evaluator_for(&self, tableau: &Tableau) -> EvaluatorKind {
        tableau.evaluator_or(self.document.evaluator)
    }

    fn temperature_for(&self, tableau: &Tableau) -> f64 {
        tableau.temperature_or(&self.document.temperature)
    }

    fn resolve_candidate(
        &self,
        value: &Value,
        tableau: &Tableau,
        span: Span,
    ) -> Result<usize, RuntimeFault> {
        match value {
            Value::Text(name) => tableau
                .candidates
                .iter()
                .position(|candidate| candidate.name == *name)
                .ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("candidate {name:?} is not declared in the selected tableau"),
                    )
                }),
            _ => {
                let index = self.exact_usize(value, span, "candidate index")?;
                (index < tableau.candidates.len())
                    .then_some(index)
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InvalidIndex,
                            span,
                            format!("candidate index {index} is outside the selected tableau"),
                        )
                    })
            }
        }
    }

    fn resolve_constraint(
        &self,
        value: &Value,
        tableau: &Tableau,
        span: Span,
    ) -> Result<usize, RuntimeFault> {
        match value {
            Value::Text(name) => tableau
                .constraints
                .iter()
                .position(|constraint| constraint.name == *name)
                .ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("constraint {name:?} is not declared in the selected tableau"),
                    )
                }),
            _ => {
                let index = self.exact_usize(value, span, "constraint index")?;
                (index < tableau.constraints.len())
                    .then_some(index)
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InvalidIndex,
                            span,
                            format!("constraint index {index} is outside the selected tableau"),
                        )
                    })
            }
        }
    }

    fn parse_evaluator(&self, value: &str, span: Span) -> Result<EvaluatorKind, RuntimeFault> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "ot" | "optimality_theory" | "optimality" => Ok(EvaluatorKind::Ot),
            "hg" | "harmonic_grammar" | "harmonicgrammar" => Ok(EvaluatorKind::HarmonicGrammar),
            "maxent" | "maximum_entropy" | "maximum_entropy_grammar" => Ok(EvaluatorKind::MaxEnt),
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("unknown evaluator {value:?}; expected OT, HG, or MaxEnt"),
            )),
        }
    }

    fn resolve_tableau_slot(&self, value: &Value, span: Span) -> Result<TableauSlot, RuntimeFault> {
        match value {
            Value::Text(value) if value.eq_ignore_ascii_case("source") => Ok(TableauSlot::Source),
            Value::Text(value) if value.eq_ignore_ascii_case("target") => Ok(TableauSlot::Target),
            Value::Text(value) => self
                .document
                .dataset
                .iter()
                .position(|tableau| tableau.name == *value)
                .map(TableauSlot::Dataset)
                .ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("no project tableau is named {value:?}"),
                    )
                }),
            _ => {
                let index = self.exact_usize(value, span, "tableau index")?;
                (index < self.document.dataset.len())
                    .then_some(TableauSlot::Dataset(index))
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InvalidIndex,
                            span,
                            format!("tableau index {index} is outside the project dataset"),
                        )
                    })
            }
        }
    }

    fn tableau_at(&self, slot: TableauSlot, span: Span) -> Result<&Tableau, RuntimeFault> {
        match slot {
            TableauSlot::Source => Ok(&self.document.source),
            TableauSlot::Target => Ok(&self.document.target),
            TableauSlot::Dataset(index) => self.document.dataset.get(index).ok_or_else(|| {
                RuntimeFault::new(
                    RuntimeDiagnosticCode::InvalidIndex,
                    span,
                    format!("tableau index {index} is outside the project dataset"),
                )
            }),
        }
    }

    fn empty_tableau(id: String, name: String, input: String) -> Tableau {
        Tableau {
            id,
            name,
            input,
            constraints: Vec::new(),
            candidates: Vec::new(),
            tie_policy: TiePolicy::RetainAll.storage_value().to_owned(),
            notes: String::new(),
            evaluator: None,
            temperature: None,
            missing_dependencies: Vec::new(),
            expected_winners: Vec::new(),
            source_locator: String::new(),
        }
    }
}

impl Runtime {
    fn call_builtin(
        &mut self,
        name: &'static str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match name {
            "print" => {
                self.arity(name, arguments, 0, usize::MAX, span)?;
                let line = arguments
                    .iter()
                    .map(Value::render)
                    .collect::<Vec<_>>()
                    .join(" ");
                let addition = line.len().saturating_add(1);
                if self.output_bytes.saturating_add(addition) > self.limits.maximum_output_bytes {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::CollectionLimit,
                        span,
                        format!(
                            "printed output exceeds the declared limit of {} bytes",
                            self.limits.maximum_output_bytes
                        ),
                    ));
                }
                self.output_bytes += addition;
                self.output.push(line);
                Ok(Value::Null)
            }
            "assert" => {
                self.arity(name, arguments, 1, 2, span)?;
                let condition = self.boolean_argument(name, arguments, 0, span)?;
                if condition {
                    Ok(Value::Boolean(true))
                } else {
                    let message = arguments
                        .get(1)
                        .map(Value::render)
                        .unwrap_or_else(|| "assertion failed".to_owned());
                    Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::AssertionFailed,
                        span,
                        message,
                    ))
                }
            }
            "assert_equal" => {
                self.arity(name, arguments, 2, 3, span)?;
                if arguments[0] == arguments[1] {
                    Ok(Value::Boolean(true))
                } else {
                    let message = arguments.get(2).map(Value::render).unwrap_or_else(|| {
                        format!(
                            "expected equal values, received {} and {}",
                            arguments[0].render(),
                            arguments[1].render()
                        )
                    });
                    Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::AssertionFailed,
                        span,
                        message,
                    ))
                }
            }
            "assert_approx" => {
                self.arity(name, arguments, 3, 4, span)?;
                let left_number = self.number_argument(name, arguments, 0, span)?.clone();
                let right_number = self.number_argument(name, arguments, 1, span)?.clone();
                let tolerance_number = self.number_argument(name, arguments, 2, span)?.clone();
                let left = self.engine_f64(&left_number, span, "assert_approx left value")?;
                let right = self.engine_f64(&right_number, span, "assert_approx right value")?;
                let tolerance =
                    self.engine_f64(&tolerance_number, span, "assert_approx tolerance")?;
                if tolerance < 0.0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "assert_approx tolerance must be nonnegative",
                    ));
                }
                if (left - right).abs() <= tolerance {
                    Ok(Value::Boolean(true))
                } else {
                    let message = arguments.get(3).map(Value::render).unwrap_or_else(|| {
                        format!("|{left} - {right}| exceeds tolerance {tolerance}")
                    });
                    Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::AssertionFailed,
                        span,
                        message,
                    ))
                }
            }
            "len" => {
                self.arity(name, arguments, 1, 1, span)?;
                let length = match &arguments[0] {
                    Value::Text(value) => value.chars().count(),
                    Value::List(value) => value.len(),
                    Value::Record(value) => value.len(),
                    value => {
                        return Err(self.type_fault(span, "text, list, or record", value, name));
                    }
                };
                Ok(Self::integer_value(length))
            }
            "range" => {
                self.arity(name, arguments, 1, 3, span)?;
                let (start, end, step) = match arguments.len() {
                    1 => (
                        0_i64,
                        self.signed_integer(&arguments[0], span, "range end")?,
                        1_i64,
                    ),
                    2 => (
                        self.signed_integer(&arguments[0], span, "range start")?,
                        self.signed_integer(&arguments[1], span, "range end")?,
                        1_i64,
                    ),
                    _ => (
                        self.signed_integer(&arguments[0], span, "range start")?,
                        self.signed_integer(&arguments[1], span, "range end")?,
                        self.signed_integer(&arguments[2], span, "range step")?,
                    ),
                };
                if step == 0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "range step cannot be zero",
                    ));
                }
                let mut values = Vec::new();
                let mut current = start;
                while (step > 0 && current < end) || (step < 0 && current > end) {
                    if values.len() >= self.limits.maximum_collection_items {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::CollectionLimit,
                            span,
                            "range exceeds the declared collection limit",
                        ));
                    }
                    values.push(Value::Number(Number::exact(BigInt::from(current))));
                    current = current.checked_add(step).ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainBoundary,
                            span,
                            "range integer overflow",
                        )
                    })?;
                }
                Ok(Value::List(values))
            }
            "type_of" => {
                self.arity(name, arguments, 1, 1, span)?;
                Ok(Value::Text(arguments[0].type_name().to_owned()))
            }
            "to_text" => {
                self.arity(name, arguments, 1, 1, span)?;
                Ok(Value::Text(arguments[0].render()))
            }
            "project_restore_v2" => {
                self.arity(name, arguments, 1, 1, span)?;
                let source = self.text_argument(name, arguments, 0, span)?;
                if source.len() > self.limits.maximum_output_bytes {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::CollectionLimit,
                        span,
                        format!(
                            "embedded v2 project has {} bytes, exceeding the declared limit of {}",
                            source.len(),
                            self.limits.maximum_output_bytes
                        ),
                    ));
                }
                let restored = document::decode(source.as_bytes()).map_err(|message| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("embedded v2 project is not formed: {message}"),
                    )
                })?;
                let item_count = std::iter::once(&restored.source)
                    .chain(std::iter::once(&restored.target))
                    .chain(restored.dataset.iter())
                    .try_fold(0_usize, |count, tableau| {
                        count
                            .checked_add(tableau.constraints.len())
                            .and_then(|value| value.checked_add(tableau.candidates.len()))
                    })
                    .and_then(|value| value.checked_add(restored.serial.moves.len()))
                    .and_then(|value| value.checked_add(restored.target_serial.moves.len()))
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::CollectionLimit,
                            span,
                            "embedded v2 project item count overflowed",
                        )
                    })?;
                self.check_collection(item_count, span)?;
                self.document = restored;
                self.selected_tableau = TableauSlot::Source;
                self.serial_side = SerialSide::Source;
                Ok(Value::Null)
            }
            "project_title" | "project_author" | "project_description" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?.to_owned();
                match name {
                    "project_title" => self.document.title = value,
                    "project_author" => self.document.author = value,
                    _ => self.document.description = value,
                }
                Ok(Value::Null)
            }
            "project_keyword" => {
                self.arity(name, arguments, 1, 1, span)?;
                let keyword = self
                    .text_argument(name, arguments, 0, span)?
                    .trim()
                    .to_owned();
                if keyword.is_empty() {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        "project keyword cannot be empty",
                    ));
                }
                if !self.document.keywords.contains(&keyword) {
                    self.document.keywords.push(keyword);
                }
                Ok(Value::Null)
            }
            "project_evaluator" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                self.document.evaluator = self.parse_evaluator(value, span)?;
                Ok(Value::Null)
            }
            "project_temperature" => {
                self.arity(name, arguments, 1, 1, span)?;
                let number = self.number_argument(name, arguments, 0, span)?.clone();
                let value = self.number_to_f64(&number, span)?;
                if value <= 0.0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "project temperature must be strictly positive",
                    ));
                }
                self.document.temperature =
                    self.scalar_from_number(&number, span, "project temperature")?;
                Ok(Value::Null)
            }
            "dataset_clear" => {
                self.arity(name, arguments, 0, 0, span)?;
                self.document.dataset.clear();
                if matches!(self.selected_tableau, TableauSlot::Dataset(_)) {
                    self.selected_tableau = TableauSlot::Source;
                }
                Ok(Value::Null)
            }
            "tableau_new" => {
                self.arity(name, arguments, 2, 2, span)?;
                let tableau_name = self.text_argument(name, arguments, 0, span)?.to_owned();
                let input = self.text_argument(name, arguments, 1, span)?.to_owned();
                if tableau_name.trim().is_empty() {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        "tableau name cannot be empty",
                    ));
                }
                if self
                    .document
                    .dataset
                    .iter()
                    .any(|item| item.name == tableau_name)
                {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("tableau name {tableau_name:?} is already in use"),
                    ));
                }
                self.check_collection(self.document.dataset.len().saturating_add(1), span)?;
                let index = self.document.dataset.len();
                let id = next_stable_id(
                    "tableau",
                    self.document
                        .dataset
                        .iter()
                        .map(|tableau| tableau.id.as_str()),
                );
                self.document
                    .dataset
                    .push(Self::empty_tableau(id, tableau_name, input));
                self.selected_tableau = TableauSlot::Dataset(index);
                Ok(Self::integer_value(index))
            }
            "tableau_select" => {
                self.arity(name, arguments, 1, 1, span)?;
                self.selected_tableau = self.resolve_tableau_slot(&arguments[0], span)?;
                Ok(Value::Null)
            }
            "tableau_copy" => {
                self.arity(name, arguments, 2, 2, span)?;
                let source = self.resolve_tableau_slot(&arguments[0], span)?;
                let target = self.resolve_tableau_slot(&arguments[1], span)?;
                let mut tableau = self.tableau_at(source, span)?.clone();
                // Copy analytical content into the target identity rather
                // than duplicating the source's stable object identity.
                tableau.id = self.tableau_at(target, span)?.id.clone();
                match target {
                    TableauSlot::Source => self.document.source = tableau,
                    TableauSlot::Target => self.document.target = tableau,
                    TableauSlot::Dataset(index) => {
                        let destination =
                            self.document.dataset.get_mut(index).ok_or_else(|| {
                                RuntimeFault::new(
                                    RuntimeDiagnosticCode::InvalidIndex,
                                    span,
                                    "target tableau disappeared",
                                )
                            })?;
                        *destination = tableau;
                    }
                }
                Ok(Value::Null)
            }
            "tableau_name" | "tableau_input" | "tableau_notes" | "tableau_source_locator" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?.to_owned();
                let tableau = self.selected_tableau_mut(span)?;
                match name {
                    "tableau_name" => tableau.name = value,
                    "tableau_input" => tableau.input = value,
                    "tableau_source_locator" => tableau.source_locator = value,
                    _ => tableau.notes = value,
                }
                Ok(Value::Null)
            }
            "tableau_evaluator" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?.to_owned();
                let evaluator = if value.eq_ignore_ascii_case("inherit") {
                    None
                } else {
                    Some(self.parse_evaluator(&value, span)?)
                };
                self.selected_tableau_mut(span)?.evaluator = evaluator;
                Ok(Value::Null)
            }
            "tableau_temperature" => {
                self.arity(name, arguments, 1, 1, span)?;
                let number = self.number_argument(name, arguments, 0, span)?.clone();
                let value = self.number_to_f64(&number, span)?;
                if value <= 0.0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "tableau temperature must be strictly positive",
                    ));
                }
                let scalar = self.scalar_from_number(&number, span, "tableau temperature")?;
                self.selected_tableau_mut(span)?.temperature = Some(scalar);
                Ok(Value::Null)
            }
            "tableau_ties" => {
                self.arity(name, arguments, 1, 1, span)?;
                let value = self.text_argument(name, arguments, 0, span)?;
                let policy = match value
                    .trim()
                    .to_ascii_lowercase()
                    .replace([' ', '-'], "_")
                    .as_str()
                {
                    "retain" | "retain_all" | "co_winners" => TiePolicy::RetainAll,
                    "first" | "first_listed" => TiePolicy::FirstListed,
                    "unique" | "require_unique" => TiePolicy::RequireUnique,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainFormation,
                            span,
                            format!("unknown tie policy {value:?}"),
                        ));
                    }
                };
                self.selected_tableau_mut(span)?.set_tie_policy(policy);
                Ok(Value::Null)
            }
            "constraints_clear" => self.builtin_constraints_clear(name, arguments, span),
            "constraint_add" | "constraint_add_unweighted" => {
                self.builtin_constraint_add(name, arguments, span)
            }
            "missing_dependency_add" => self.builtin_missing_dependency_add(name, arguments, span),
            "constraint_remove" => self.builtin_constraint_remove(name, arguments, span),
            "constraint_move" => self.builtin_constraint_move(name, arguments, span),
            "constraint_rank"
            | "constraint_tie"
            | "constraint_weight"
            | "constraint_definition"
            | "constraint_enabled"
            | "constraint_prior" => self.builtin_constraint_property(name, arguments, span),
            "candidates_clear" => self.builtin_candidates_clear(name, arguments, span),
            "candidate_add" => self.builtin_candidate_add(name, arguments, span),
            "candidate_add_structured" => {
                self.builtin_candidate_add_structured(name, arguments, span)
            }
            "candidate_remove" | "candidate_move" | "candidate_name" | "candidate_form"
            | "candidate_mass" | "candidate_observed" | "candidate_notes" => {
                self.builtin_candidate_operation(name, arguments, span)
            }
            "violation_set" | "violation_get" => self.builtin_violation(name, arguments, span),
            "evaluate"
            | "winners"
            | "winning_forms"
            | "harmony"
            | "probability"
            | "assert_winners"
            | "assert_winning_forms"
            | "assert_probability" => self.builtin_evaluation(name, arguments, span),
            "maxent_learn" | "infer_ranking" | "harmonic_bounds" | "unnecessary_constraints" => {
                self.builtin_learning(name, arguments, span)
            }
            "serial_side" | "serial_start" | "serial_limit" | "serial_clear" | "serial_move"
            | "serial_evaluate" => self.builtin_serial(name, arguments, span),
            "second_query"
            | "second_answer_sort"
            | "second_scope"
            | "second_transformation"
            | "second_transport"
            | "second_layout"
            | "second_mode"
            | "second_tolerance"
            | "second_grid_step"
            | "second_response_domain"
            | "second_normalizer"
            | "second_layers"
            | "second_layer_transport"
            | "second_consumer"
            | "second_compare" => self.builtin_second_order(name, arguments, span),
            "q_ranking_space" | "q_clone" | "typology" => self.builtin_q(name, arguments, span),
            "mark_data" | "constraint_demotion" | "partial_ranking_extensions" => {
                self.builtin_ranking(name, arguments, span)
            }
            "generator_identity"
            | "generator_delete"
            | "generator_insert"
            | "generator_substitute"
            | "generator_swap"
            | "segments"
            | "unique"
            | "candidates_from_forms"
            | "phonological_form"
            | "finite_gen"
            | "generation_to_tableau" => self.builtin_generation(name, arguments, span),
            "save" | "export_tableau" | "export_plot" => self.builtin_file(name, arguments, span),
            _ => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                format!("builtin {name:?} is registered but not implemented"),
            )),
        }
    }

    fn signed_integer(
        &self,
        value: &Value,
        span: Span,
        purpose: &str,
    ) -> Result<i64, RuntimeFault> {
        let Value::Number(Number::Exact(number)) = value else {
            return Err(self.type_fault(span, "exact integer", value, purpose));
        };
        if !number.is_integer() {
            return Err(self.type_fault(span, "exact integer", value, purpose));
        }
        number.to_integer().to_i64().ok_or_else(|| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!("{purpose} is outside the supported signed integer range"),
            )
        })
    }
}

// ---------------------------------------------------------------------------
// Tableau construction

impl Runtime {
    fn builtin_constraints_clear(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 0, 0, span)?;
        let slot = self.selected_tableau;
        let tableau = self.selected_tableau_mut(span)?;
        tableau.constraints.clear();
        for candidate in &mut tableau.candidates {
            candidate.violations.clear();
        }
        match slot {
            TableauSlot::Source => {
                for movement in &mut self.document.serial.moves {
                    movement.violations.clear();
                }
            }
            TableauSlot::Target => {
                for movement in &mut self.document.target_serial.moves {
                    movement.violations.clear();
                }
            }
            TableauSlot::Dataset(_) => {}
        }
        Ok(Value::Null)
    }

    fn builtin_constraint_add(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let unweighted = name == "constraint_add_unweighted";
        self.arity(name, arguments, 1, if unweighted { 3 } else { 4 }, span)?;
        let constraint_name = self
            .text_argument(name, arguments, 0, span)?
            .trim()
            .to_owned();
        if constraint_name.is_empty() {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                "constraint name cannot be empty",
            ));
        }
        {
            let tableau = self.selected_tableau(span)?;
            let serial_moves = match self.selected_tableau {
                TableauSlot::Source => self.document.serial.moves.len(),
                TableauSlot::Target => self.document.target_serial.moves.len(),
                TableauSlot::Dataset(_) => 0,
            };
            if !tableau.candidates.is_empty() || serial_moves != 0 {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainBoundary,
                    span,
                    format!(
                        "{name} cannot invent marks for existing candidates or serial moves; declare the constraint register before the phonologist-supplied violation ledger"
                    ),
                ));
            }
        }
        let (weight, weight_center) = if unweighted {
            (None, None)
        } else if let Some(value) = arguments.get(1) {
            let Value::Number(value) = value else {
                return Err(self.type_fault(span, "number", value, "constraint weight"));
            };
            (
                Some(self.scalar_from_number(value, span, "constraint weight")?),
                Some(self.number_to_f64(value, span)?),
            )
        } else {
            (Some(NumericScalar::integer(1)), Some(1.0))
        };
        let definition_index = if unweighted { 1 } else { 2 };
        let stratum_index = if unweighted { 2 } else { 3 };
        let definition = if let Some(Value::Text(value)) = arguments.get(definition_index) {
            value.clone()
        } else if let Some(value) = arguments.get(definition_index) {
            return Err(self.type_fault(span, "text", value, "constraint definition"));
        } else {
            String::new()
        };
        let default_stratum = self.selected_tableau(span)?.constraints.len();
        let stratum = if let Some(value) = arguments.get(stratum_index) {
            self.exact_usize(value, span, "constraint stratum")?
        } else {
            default_stratum
        };
        let evaluator = {
            let tableau = self.selected_tableau(span)?;
            self.evaluator_for(tableau)
        };
        if weight_center.is_some_and(|weight| weight < 0.0) && evaluator != EvaluatorKind::Ot {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                "enabled HG and MaxEnt constraints require nonnegative weights",
            ));
        }
        let tableau = self.selected_tableau_mut(span)?;
        if tableau
            .constraints
            .iter()
            .any(|item| item.name == constraint_name)
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("constraint name {constraint_name:?} is already in use"),
            ));
        }
        let index = tableau.constraints.len();
        let id = next_stable_id(
            "constraint",
            tableau
                .constraints
                .iter()
                .map(|constraint| constraint.id.as_str()),
        );
        tableau.constraints.push(Constraint {
            id,
            name: constraint_name,
            weight,
            stratum,
            enabled: true,
            definition,
            prior_mean: NumericScalar::integer(0),
            prior_sigma: NumericScalar::integer(100_000),
        });
        Ok(Self::integer_value(index))
    }

    fn builtin_missing_dependency_add(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 6, 7, span)?;
        let code = self
            .text_argument(name, arguments, 0, span)?
            .trim()
            .to_owned();
        let stage_text = self.text_argument(name, arguments, 1, span)?;
        let scope_text = self.text_argument(name, arguments, 2, span)?;
        let coordinate = self
            .text_argument(name, arguments, 3, span)?
            .trim()
            .to_owned();
        let message = self
            .text_argument(name, arguments, 4, span)?
            .trim()
            .to_owned();
        let remedy = self
            .text_argument(name, arguments, 5, span)?
            .trim()
            .to_owned();
        let normalize = |value: &str| value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        let stage = match normalize(stage_text).as_str() {
            "formation" => DependencyStage::Formation,
            "admission" => DependencyStage::Admission,
            _ => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!(
                        "unknown missing-dependency stage {stage_text:?}; expected formation or admission"
                    ),
                ));
            }
        };
        let scope = match normalize(scope_text).as_str() {
            "any_evaluation" if arguments.len() == 6 => DependencyScope::AnyEvaluation,
            "learning" if arguments.len() == 6 => DependencyScope::Learning,
            "exact_certification" if arguments.len() == 6 => DependencyScope::ExactCertification,
            "evaluator" if arguments.len() == 7 => {
                let evaluator = self.text_argument(name, arguments, 6, span)?;
                DependencyScope::Evaluator {
                    evaluator: self.parse_evaluator(evaluator, span)?,
                }
            }
            "evaluator" => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    "evaluator-scoped missing_dependency_add requires argument 7 naming OT, HG, or MaxEnt",
                ));
            }
            "any_evaluation" | "learning" | "exact_certification" => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    "missing_dependency_add accepts argument 7 only for evaluator scope",
                ));
            }
            _ => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainFormation,
                    span,
                    format!(
                        "unknown missing-dependency scope {scope_text:?}; expected any_evaluation, evaluator, learning, or exact_certification"
                    ),
                ));
            }
        };
        let dependency = MissingDependency {
            code,
            stage,
            coordinate,
            scope,
            message,
            remedy,
        };
        dependency.validate().map_err(|message| {
            RuntimeFault::new(RuntimeDiagnosticCode::DomainFormation, span, message)
        })?;
        let next_len = self
            .selected_tableau(span)?
            .missing_dependencies
            .len()
            .saturating_add(1);
        self.check_collection(next_len, span)?;
        let tableau = self.selected_tableau_mut(span)?;
        if tableau.missing_dependencies.iter().any(|existing| {
            existing.code == dependency.code
                && existing.coordinate == dependency.coordinate
                && existing.scope == dependency.scope
        }) {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!(
                    "missing dependency {} at {} is already declared for this scope",
                    dependency.code, dependency.coordinate
                ),
            ));
        }
        tableau.missing_dependencies.push(dependency);
        Ok(Value::Null)
    }

    fn builtin_constraint_remove(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 1, 1, span)?;
        let index = {
            let tableau = self.selected_tableau(span)?;
            self.resolve_constraint(&arguments[0], tableau, span)?
        };
        let slot = self.selected_tableau;
        let tableau = self.selected_tableau_mut(span)?;
        tableau.constraints.remove(index);
        for candidate in &mut tableau.candidates {
            if index < candidate.violations.len() {
                candidate.violations.remove(index);
            }
        }
        match slot {
            TableauSlot::Source => {
                for movement in &mut self.document.serial.moves {
                    if index < movement.violations.len() {
                        movement.violations.remove(index);
                    }
                }
            }
            TableauSlot::Target => {
                for movement in &mut self.document.target_serial.moves {
                    if index < movement.violations.len() {
                        movement.violations.remove(index);
                    }
                }
            }
            TableauSlot::Dataset(_) => {}
        }
        Ok(Value::Null)
    }

    fn builtin_constraint_move(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 2, 2, span)?;
        let (from, to, width) = {
            let tableau = self.selected_tableau(span)?;
            (
                self.resolve_constraint(&arguments[0], tableau, span)?,
                self.exact_usize(&arguments[1], span, "constraint destination")?,
                tableau.constraints.len(),
            )
        };
        if to >= width {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InvalidIndex,
                span,
                format!("constraint destination {to} is outside width {width}"),
            ));
        }
        let slot = self.selected_tableau;
        let serial_marks_are_rectangular = match slot {
            TableauSlot::Source => self
                .document
                .serial
                .moves
                .iter()
                .all(|movement| movement.violations.len() == width),
            TableauSlot::Target => self
                .document
                .target_serial
                .moves
                .iter()
                .all(|movement| movement.violations.len() == width),
            TableauSlot::Dataset(_) => true,
        };
        if !serial_marks_are_rectangular {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                "cannot move a constraint while a registered serial move has a nonrectangular violation vector",
            ));
        }
        let tableau = self.selected_tableau_mut(span)?;
        let constraint = tableau.constraints.remove(from);
        tableau.constraints.insert(to, constraint);
        for (stratum, constraint) in tableau.constraints.iter_mut().enumerate() {
            constraint.stratum = stratum;
        }
        for candidate in &mut tableau.candidates {
            let mark = candidate.violations.remove(from);
            candidate.violations.insert(to, mark);
        }
        let serial = match slot {
            TableauSlot::Source => Some(&mut self.document.serial),
            TableauSlot::Target => Some(&mut self.document.target_serial),
            TableauSlot::Dataset(_) => None,
        };
        if let Some(serial) = serial {
            for movement in &mut serial.moves {
                let mark = movement.violations.remove(from);
                movement.violations.insert(to, mark);
            }
        }
        Ok(Value::Null)
    }

    fn builtin_constraint_property(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let expected = if name == "constraint_prior" || name == "constraint_tie" {
            3_usize.min(if name == "constraint_tie" { 2 } else { 3 })
        } else {
            2
        };
        self.arity(name, arguments, expected, expected, span)?;
        let index = {
            let tableau = self.selected_tableau(span)?;
            self.resolve_constraint(&arguments[0], tableau, span)?
        };
        match name {
            "constraint_rank" => {
                let stratum = self.exact_usize(&arguments[1], span, "constraint stratum")?;
                self.selected_tableau_mut(span)?.constraints[index].stratum = stratum;
            }
            "constraint_tie" => {
                let other = {
                    let tableau = self.selected_tableau(span)?;
                    self.resolve_constraint(&arguments[1], tableau, span)?
                };
                let stratum = self.selected_tableau(span)?.constraints[other].stratum;
                self.selected_tableau_mut(span)?.constraints[index].stratum = stratum;
            }
            "constraint_weight" => {
                let Value::Number(number) = &arguments[1] else {
                    return Err(self.type_fault(
                        span,
                        "number",
                        &arguments[1],
                        "constraint weight",
                    ));
                };
                let weight = self.number_to_f64(number, span)?;
                let evaluator = {
                    let tableau = self.selected_tableau(span)?;
                    self.evaluator_for(tableau)
                };
                if weight < 0.0 && evaluator != EvaluatorKind::Ot {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "enabled HG and MaxEnt constraints require nonnegative weights",
                    ));
                }
                let scalar = self.scalar_from_number(number, span, "constraint weight")?;
                self.selected_tableau_mut(span)?.constraints[index].weight = Some(scalar);
            }
            "constraint_definition" => {
                let value = self.text_argument(name, arguments, 1, span)?.to_owned();
                self.selected_tableau_mut(span)?.constraints[index].definition = value;
            }
            "constraint_enabled" => {
                let value = self.boolean_argument(name, arguments, 1, span)?;
                self.selected_tableau_mut(span)?.constraints[index].enabled = value;
            }
            "constraint_prior" => {
                let mean_number = self.number_argument(name, arguments, 1, span)?.clone();
                let sigma_number = self.number_argument(name, arguments, 2, span)?.clone();
                let sigma = self.number_to_f64(&sigma_number, span)?;
                if sigma <= 0.0 {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "constraint prior sigma must be strictly positive",
                    ));
                }
                let mean_scalar =
                    self.scalar_from_number(&mean_number, span, "constraint prior mean")?;
                let sigma_scalar =
                    self.scalar_from_number(&sigma_number, span, "constraint prior sigma")?;
                let constraint = &mut self.selected_tableau_mut(span)?.constraints[index];
                constraint.prior_mean = mean_scalar;
                constraint.prior_sigma = sigma_scalar;
            }
            _ => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    span,
                    format!("unimplemented constraint property {name}"),
                ));
            }
        }
        Ok(Value::Null)
    }

    fn builtin_candidates_clear(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 0, 0, span)?;
        self.selected_tableau_mut(span)?.candidates.clear();
        Ok(Value::Null)
    }

    fn builtin_candidate_add(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 3, 5, span)?;
        let candidate_name = self
            .text_argument(name, arguments, 0, span)?
            .trim()
            .to_owned();
        let form = self.text_argument(name, arguments, 1, span)?.to_owned();
        if candidate_name.is_empty() {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                "candidate identity cannot be empty",
            ));
        }
        let width = self.selected_tableau(span)?.constraints.len();
        let value = &arguments[2];
        let Value::List(values) = value else {
            return Err(self.type_fault(span, "list", value, "candidate violation profile"));
        };
        if values.len() != width {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!(
                    "candidate has {} marks for {width} constraints",
                    values.len()
                ),
            ));
        }
        let violations = values
            .iter()
            .map(|value| self.exact_u16(value, span, "violation mark"))
            .collect::<Result<Vec<_>, _>>()?;
        let (base_mass, base_mass_center) = if let Some(Value::Number(number)) = arguments.get(3) {
            (
                self.scalar_from_number(number, span, "candidate base mass")?,
                self.number_to_f64(number, span)?,
            )
        } else if let Some(value) = arguments.get(3) {
            return Err(self.type_fault(span, "number", value, "candidate base mass"));
        } else {
            (NumericScalar::integer(1), 1.0)
        };
        let (observed_frequency, observed_center) =
            if let Some(Value::Number(number)) = arguments.get(4) {
                (
                    self.scalar_from_number(number, span, "candidate observed frequency")?,
                    self.number_to_f64(number, span)?,
                )
            } else if let Some(value) = arguments.get(4) {
                return Err(self.type_fault(span, "number", value, "candidate observed frequency"));
            } else {
                (NumericScalar::integer(0), 0.0)
            };
        if base_mass_center <= 0.0 || observed_center < 0.0 {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                "candidate base mass must be positive and observed frequency nonnegative",
            ));
        }
        let tableau = self.selected_tableau_mut(span)?;
        if tableau
            .candidates
            .iter()
            .any(|item| item.name == candidate_name)
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("candidate identity {candidate_name:?} is already in use"),
            ));
        }
        let index = tableau.candidates.len();
        let id = next_stable_id(
            "candidate",
            tableau
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str()),
        );
        tableau.candidates.push(Candidate {
            id,
            name: candidate_name,
            form,
            violations,
            base_mass,
            notes: String::new(),
            observed_frequency,
            structured: None,
        });
        Ok(Self::integer_value(index))
    }

    fn builtin_candidate_add_structured(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(name, arguments, 3, 5, span)?;
        let candidate_name = self
            .text_argument(name, arguments, 0, span)?
            .trim()
            .to_owned();
        if candidate_name.is_empty() {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                "candidate identity cannot be empty",
            ));
        }
        let structured =
            structured_candidate_from_value(&arguments[1], "candidate.structured", span)?;
        let form = structured.surface_string();
        let width = self.selected_tableau(span)?.constraints.len();
        let Value::List(values) = &arguments[2] else {
            return Err(self.type_fault(
                span,
                "list",
                &arguments[2],
                "candidate violation profile",
            ));
        };
        if values.len() != width {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!(
                    "candidate has {} marks for {width} constraints",
                    values.len()
                ),
            ));
        }
        let violations = values
            .iter()
            .map(|value| self.exact_u16(value, span, "violation mark"))
            .collect::<Result<Vec<_>, _>>()?;
        let (base_mass, base_mass_center) = if let Some(Value::Number(number)) = arguments.get(3) {
            (
                self.scalar_from_number(number, span, "candidate base mass")?,
                self.number_to_f64(number, span)?,
            )
        } else if let Some(value) = arguments.get(3) {
            return Err(self.type_fault(span, "number", value, "candidate base mass"));
        } else {
            (NumericScalar::integer(1), 1.0)
        };
        let (observed_frequency, observed_center) =
            if let Some(Value::Number(number)) = arguments.get(4) {
                (
                    self.scalar_from_number(number, span, "candidate observed frequency")?,
                    self.number_to_f64(number, span)?,
                )
            } else if let Some(value) = arguments.get(4) {
                return Err(self.type_fault(span, "number", value, "candidate observed frequency"));
            } else {
                (NumericScalar::integer(0), 0.0)
            };
        if base_mass_center <= 0.0 || observed_center < 0.0 {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                "candidate base mass must be positive and observed frequency nonnegative",
            ));
        }
        let tableau = self.selected_tableau_mut(span)?;
        if tableau
            .candidates
            .iter()
            .any(|item| item.name == candidate_name)
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainFormation,
                span,
                format!("candidate identity {candidate_name:?} is already in use"),
            ));
        }
        let index = tableau.candidates.len();
        let id = next_stable_id(
            "candidate",
            tableau
                .candidates
                .iter()
                .map(|candidate| candidate.id.as_str()),
        );
        tableau.candidates.push(Candidate {
            id,
            name: candidate_name,
            form,
            violations,
            base_mass,
            notes: String::new(),
            observed_frequency,
            structured: Some(structured),
        });
        Ok(Self::integer_value(index))
    }

    fn builtin_candidate_operation(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(
            name,
            arguments,
            if name == "candidate_remove" { 1 } else { 2 },
            if name == "candidate_remove" { 1 } else { 2 },
            span,
        )?;
        let index = {
            let tableau = self.selected_tableau(span)?;
            self.resolve_candidate(&arguments[0], tableau, span)?
        };
        match name {
            "candidate_remove" => {
                self.selected_tableau_mut(span)?.candidates.remove(index);
            }
            "candidate_move" => {
                let to = self.exact_usize(&arguments[1], span, "candidate destination")?;
                let length = self.selected_tableau(span)?.candidates.len();
                if to >= length {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::InvalidIndex,
                        span,
                        format!("candidate destination {to} is outside length {length}"),
                    ));
                }
                let tableau = self.selected_tableau_mut(span)?;
                let candidate = tableau.candidates.remove(index);
                tableau.candidates.insert(to, candidate);
            }
            "candidate_name" => {
                let value = self
                    .text_argument(name, arguments, 1, span)?
                    .trim()
                    .to_owned();
                if value.is_empty() {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        "candidate identity cannot be empty",
                    ));
                }
                if self
                    .selected_tableau(span)?
                    .candidates
                    .iter()
                    .enumerate()
                    .any(|(other, candidate)| other != index && candidate.name == value)
                {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainFormation,
                        span,
                        format!("candidate identity {value:?} is already in use"),
                    ));
                }
                self.selected_tableau_mut(span)?.candidates[index].name = value;
            }
            "candidate_form" | "candidate_notes" => {
                let value = self.text_argument(name, arguments, 1, span)?.to_owned();
                let candidate = &mut self.selected_tableau_mut(span)?.candidates[index];
                if name == "candidate_form" {
                    candidate.form = value;
                } else {
                    candidate.notes = value;
                }
            }
            "candidate_mass" | "candidate_observed" => {
                let number = self.number_argument(name, arguments, 1, span)?.clone();
                let value = self.number_to_f64(&number, span)?;
                if (name == "candidate_mass" && value <= 0.0)
                    || (name == "candidate_observed" && value < 0.0)
                {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        if name == "candidate_mass" {
                            "candidate base mass must be strictly positive"
                        } else {
                            "candidate observed frequency must be nonnegative"
                        },
                    ));
                }
                let scalar = self.scalar_from_number(&number, span, name)?;
                let candidate = &mut self.selected_tableau_mut(span)?.candidates[index];
                if name == "candidate_mass" {
                    candidate.base_mass = scalar;
                } else {
                    candidate.observed_frequency = scalar;
                }
            }
            _ => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::InternalState,
                    span,
                    format!("unimplemented candidate operation {name}"),
                ));
            }
        }
        Ok(Value::Null)
    }

    fn builtin_violation(
        &mut self,
        name: &str,
        arguments: &[Value],
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.arity(
            name,
            arguments,
            if name == "violation_get" { 2 } else { 3 },
            if name == "violation_get" { 2 } else { 3 },
            span,
        )?;
        let (candidate, constraint) = {
            let tableau = self.selected_tableau(span)?;
            (
                self.resolve_candidate(&arguments[0], tableau, span)?,
                self.resolve_constraint(&arguments[1], tableau, span)?,
            )
        };
        if name == "violation_get" {
            let value = self.selected_tableau(span)?.candidates[candidate].violations[constraint];
            Ok(Self::integer_value(usize::from(value)))
        } else {
            let value = self.exact_u16(&arguments[2], span, "violation mark")?;
            self.selected_tableau_mut(span)?.candidates[candidate].violations[constraint] = value;
            Ok(Value::Null)
        }
    }
}

// ---------------------------------------------------------------------------
// Expressions and calls

impl Runtime {
    fn evaluate_expression(
        &mut self,
        expression: &Expression,
        environment: EnvironmentRef,
    ) -> Result<Value, RuntimeFault> {
        self.tick(expression.span)?;
        self.statistics.expressions = self.statistics.expressions.saturating_add(1);
        match &expression.kind {
            ExpressionKind::Literal(literal) => self.literal(literal, expression.span),
            ExpressionKind::Variable(name) => {
                match self.lookup(&environment, name, expression.span)?.value {
                    BindingValue::Data(value) => Ok(value),
                    BindingValue::Function(_) | BindingValue::Builtin(_) => Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::Type,
                        expression.span,
                        format!("callable {name:?} must be invoked with parentheses"),
                    )),
                }
            }
            ExpressionKind::List(expressions) => {
                self.check_collection(expressions.len(), expression.span)?;
                expressions
                    .iter()
                    .map(|item| self.evaluate_expression(item, environment.clone()))
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::List)
            }
            ExpressionKind::Record(entries) => {
                self.check_collection(entries.len(), expression.span)?;
                let mut values = BTreeMap::new();
                for entry in entries {
                    self.check_collection(entry.key.chars().count(), entry.key_span)?;
                    let value = self.evaluate_expression(&entry.value, environment.clone())?;
                    if values.insert(entry.key.clone(), value).is_some() {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::DuplicateName,
                            entry.key_span,
                            format!("duplicate record key {:?}", entry.key),
                        ));
                    }
                }
                Ok(Value::Record(values))
            }
            ExpressionKind::Group(inner) => self.evaluate_expression(inner, environment),
            ExpressionKind::Unary { operator, operand } => {
                let value = self.evaluate_expression(operand, environment)?;
                self.unary(*operator, value, expression.span)
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_expression(left, environment.clone())?;
                if *operator == BinaryOperator::And {
                    if !self.boolean(left, expression.span)? {
                        return Ok(Value::Boolean(false));
                    }
                    let right = self.evaluate_expression(right, environment)?;
                    return Ok(Value::Boolean(self.boolean(right, expression.span)?));
                }
                if *operator == BinaryOperator::Or {
                    if self.boolean(left, expression.span)? {
                        return Ok(Value::Boolean(true));
                    }
                    let right = self.evaluate_expression(right, environment)?;
                    return Ok(Value::Boolean(self.boolean(right, expression.span)?));
                }
                let right = self.evaluate_expression(right, environment)?;
                self.binary(*operator, left, right, expression.span)
            }
            ExpressionKind::Assignment { name, value, .. } => {
                let value = self.evaluate_expression(value, environment.clone())?;
                self.assign(&environment, name, value, expression.span)
            }
            ExpressionKind::Call { callee, arguments } => {
                let ExpressionKind::Variable(name) = &callee.kind else {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::NotCallable,
                        callee.span,
                        "PhonoScript calls require a named function or builtin",
                    ));
                };
                self.check_collection(arguments.len(), expression.span)?;
                let arguments = arguments
                    .iter()
                    .map(|argument| self.evaluate_expression(argument, environment.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                self.call(name, arguments, environment, expression.span)
            }
            ExpressionKind::Index { collection, index } => {
                let collection = self.evaluate_expression(collection, environment.clone())?;
                let index = self.evaluate_expression(index, environment)?;
                self.index(collection, index, expression.span)
            }
            ExpressionKind::Member {
                object,
                field,
                field_span,
            } => {
                let object = self.evaluate_expression(object, environment)?;
                let Value::Record(values) = object else {
                    return Err(self.type_fault(*field_span, "record", &object, "member access"));
                };
                values.get(field).cloned().ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InvalidIndex,
                        *field_span,
                        format!("record has no field {field:?}"),
                    )
                })
            }
        }
    }

    fn literal(&self, literal: &Literal, span: Span) -> Result<Value, RuntimeFault> {
        match literal {
            Literal::Number(number) => {
                let exact = match number {
                    NumericLiteral::Integer(value) => BigRational::from_integer(value.clone()),
                    NumericLiteral::Rational(value) | NumericLiteral::Decimal { value, .. } => {
                        value.clone()
                    }
                };
                self.check_exact(&exact, span)?;
                Ok(Value::Number(Number::Exact(exact)))
            }
            Literal::Boolean(value) => Ok(Value::Boolean(*value)),
            Literal::Text(value) => {
                self.check_collection(value.chars().count(), span)?;
                Ok(Value::Text(value.clone()))
            }
            Literal::Null => Ok(Value::Null),
        }
    }

    fn unary(
        &self,
        operator: UnaryOperator,
        value: Value,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match operator {
            UnaryOperator::Not => Ok(Value::Boolean(!self.boolean(value, span)?)),
            UnaryOperator::Positive => match value {
                Value::Number(_) => Ok(value),
                value => Err(self.type_fault(span, "number", &value, "unary +")),
            },
            UnaryOperator::Negate => match value {
                Value::Number(Number::Exact(value)) => {
                    let value = -value;
                    self.check_exact(&value, span)?;
                    Ok(Value::Number(Number::Exact(value)))
                }
                Value::Number(Number::Approximate(value)) => Number::finite_approximate(-value)
                    .map(Value::Number)
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::DomainBoundary,
                            span,
                            "approximate negation produced a nonfinite value",
                        )
                    }),
                value => Err(self.type_fault(span, "number", &value, "unary -")),
            },
        }
    }

    fn binary(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        match operator {
            BinaryOperator::Equal => Ok(Value::Boolean(left == right)),
            BinaryOperator::NotEqual => Ok(Value::Boolean(left != right)),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => self.compare(operator, left, right, span),
            BinaryOperator::Add => match (left, right) {
                (Value::Text(mut left), Value::Text(right)) => {
                    self.check_collection(left.chars().count() + right.chars().count(), span)?;
                    left.push_str(&right);
                    Ok(Value::Text(left))
                }
                (Value::List(mut left), Value::List(right)) => {
                    self.check_collection(left.len() + right.len(), span)?;
                    left.extend(right);
                    Ok(Value::List(left))
                }
                (Value::Number(left), Value::Number(right)) => self
                    .number_binary(operator, left, right, span)
                    .map(Value::Number),
                (left, right) => Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::Type,
                    span,
                    format!(
                        "+ expects two numbers, two texts, or two lists; received {} and {}",
                        left.type_name(),
                        right.type_name()
                    ),
                )),
            },
            BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => match (left, right) {
                (Value::Number(left), Value::Number(right)) => self
                    .number_binary(operator, left, right, span)
                    .map(Value::Number),
                (left, right) => Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::Type,
                    span,
                    format!(
                        "numeric operator expects two numbers; received {} and {}",
                        left.type_name(),
                        right.type_name()
                    ),
                )),
            },
            BinaryOperator::And | BinaryOperator::Or => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::InternalState,
                span,
                "short-circuit operator reached the eager binary evaluator",
            )),
        }
    }

    fn number_binary(
        &self,
        operator: BinaryOperator,
        left: Number,
        right: Number,
        span: Span,
    ) -> Result<Number, RuntimeFault> {
        if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && right.is_zero()
        {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DivisionByZero,
                span,
                "division or remainder by zero",
            ));
        }
        match (left, right) {
            (Number::Exact(left), Number::Exact(right)) => {
                let value = match operator {
                    BinaryOperator::Add => left + right,
                    BinaryOperator::Subtract => left - right,
                    BinaryOperator::Multiply => left * right,
                    BinaryOperator::Divide => left / right,
                    BinaryOperator::Remainder => {
                        if !left.is_integer() || !right.is_integer() {
                            return Err(RuntimeFault::new(
                                RuntimeDiagnosticCode::Type,
                                span,
                                "remainder requires exact integer operands",
                            ));
                        }
                        BigRational::from_integer(left.to_integer() % right.to_integer())
                    }
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::InternalState,
                            span,
                            "non-arithmetic operator reached number arithmetic",
                        ));
                    }
                };
                self.check_exact(&value, span)?;
                Ok(Number::Exact(value))
            }
            (left, right) => {
                let left = self.number_to_f64(&left, span)?;
                let right = self.number_to_f64(&right, span)?;
                let value = match operator {
                    BinaryOperator::Add => left + right,
                    BinaryOperator::Subtract => left - right,
                    BinaryOperator::Multiply => left * right,
                    BinaryOperator::Divide => left / right,
                    BinaryOperator::Remainder => left % right,
                    _ => {
                        return Err(RuntimeFault::new(
                            RuntimeDiagnosticCode::InternalState,
                            span,
                            "non-arithmetic operator reached approximate arithmetic",
                        ));
                    }
                };
                Number::finite_approximate(value).ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::DomainBoundary,
                        span,
                        "approximate arithmetic produced a nonfinite value",
                    )
                })
            }
        }
    }

    fn compare(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        let ordering = match (&left, &right) {
            (Value::Number(Number::Exact(left)), Value::Number(Number::Exact(right))) => {
                left.cmp(right)
            }
            (
                Value::Number(Number::Approximate(left)),
                Value::Number(Number::Approximate(right)),
            ) => left.total_cmp(right),
            (Value::Number(_), Value::Number(_)) => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::DomainBoundary,
                    span,
                    "ordering an exact number against an approximate number requires an explicit common approximation boundary",
                ));
            }
            (Value::Text(left), Value::Text(right)) => left.cmp(right),
            _ => {
                return Err(RuntimeFault::new(
                    RuntimeDiagnosticCode::Type,
                    span,
                    format!(
                        "ordering requires two numbers or two texts; received {} and {}",
                        left.type_name(),
                        right.type_name()
                    ),
                ));
            }
        };
        let answer = match operator {
            BinaryOperator::Less => ordering.is_lt(),
            BinaryOperator::LessEqual => !ordering.is_gt(),
            BinaryOperator::Greater => ordering.is_gt(),
            BinaryOperator::GreaterEqual => !ordering.is_lt(),
            _ => false,
        };
        Ok(Value::Boolean(answer))
    }

    fn boolean(&self, value: Value, span: Span) -> Result<bool, RuntimeFault> {
        match value {
            Value::Boolean(value) => Ok(value),
            value => Err(self.type_fault(span, "boolean", &value, "condition")),
        }
    }

    fn number_to_f64(&self, number: &Number, span: Span) -> Result<f64, RuntimeFault> {
        let value = match number {
            Number::Exact(value) => value.to_f64(),
            Number::Approximate(value) => Some(*value),
        }
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                "number is outside the finite floating-point engine boundary",
            )
        })?;
        Ok(value)
    }

    fn exact_usize(&self, value: &Value, span: Span, purpose: &str) -> Result<usize, RuntimeFault> {
        let Value::Number(Number::Exact(number)) = value else {
            return Err(self.type_fault(span, "exact nonnegative integer", value, purpose));
        };
        if !number.is_integer() || number.is_negative() {
            return Err(self.type_fault(span, "exact nonnegative integer", value, purpose));
        }
        number.to_integer().to_usize().ok_or_else(|| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!("{purpose} is outside the supported integer range"),
            )
        })
    }

    fn exact_u16(&self, value: &Value, span: Span, purpose: &str) -> Result<u16, RuntimeFault> {
        let index = self.exact_usize(value, span, purpose)?;
        let mark = u16::try_from(index).map_err(|_| {
            RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!("{purpose} exceeds the maximum violation mark {MAX_VIOLATION}"),
            )
        })?;
        if mark > MAX_VIOLATION {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::DomainBoundary,
                span,
                format!("{purpose} exceeds the maximum violation mark {MAX_VIOLATION}"),
            ));
        }
        Ok(mark)
    }

    fn check_exact(&self, value: &BigRational, span: Span) -> Result<(), RuntimeFault> {
        let bytes =
            value.numer().to_signed_bytes_be().len() + value.denom().to_signed_bytes_be().len();
        if bytes > self.limits.maximum_exact_bytes {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::NumericLimit,
                span,
                format!(
                    "exact value requires {bytes} bytes, above the declared limit of {}",
                    self.limits.maximum_exact_bytes
                ),
            ));
        }
        Ok(())
    }

    fn check_collection(&self, length: usize, span: Span) -> Result<(), RuntimeFault> {
        if length > self.limits.maximum_collection_items {
            return Err(RuntimeFault::new(
                RuntimeDiagnosticCode::CollectionLimit,
                span,
                format!(
                    "collection has {length} items, above the declared limit of {}",
                    self.limits.maximum_collection_items
                ),
            ));
        }
        Ok(())
    }

    fn index(&self, collection: Value, index: Value, span: Span) -> Result<Value, RuntimeFault> {
        match collection {
            Value::List(values) => {
                let index = self.exact_usize(&index, span, "list index")?;
                values.get(index).cloned().ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InvalidIndex,
                        span,
                        format!(
                            "list index {index} is outside a list of length {}",
                            values.len()
                        ),
                    )
                })
            }
            Value::Text(text) => {
                let index = self.exact_usize(&index, span, "text index")?;
                text.chars()
                    .nth(index)
                    .map(|value| Value::Text(value.to_string()))
                    .ok_or_else(|| {
                        RuntimeFault::new(
                            RuntimeDiagnosticCode::InvalidIndex,
                            span,
                            format!("text index {index} is outside the text"),
                        )
                    })
            }
            Value::Record(values) => {
                let Value::Text(index) = index else {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::Type,
                        span,
                        "record index must be text",
                    ));
                };
                values.get(&index).cloned().ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InvalidIndex,
                        span,
                        format!("record has no field {index:?}"),
                    )
                })
            }
            value => Err(self.type_fault(span, "list, text, or record", &value, "indexing")),
        }
    }

    fn type_fault(
        &self,
        span: Span,
        expected: &str,
        actual: &Value,
        purpose: &str,
    ) -> RuntimeFault {
        RuntimeFault::new(
            RuntimeDiagnosticCode::Type,
            span,
            format!(
                "{purpose} requires {expected}; received {}",
                actual.type_name()
            ),
        )
    }

    fn call(
        &mut self,
        name: &str,
        arguments: Vec<Value>,
        environment: EnvironmentRef,
        span: Span,
    ) -> Result<Value, RuntimeFault> {
        self.tick(span)?;
        self.statistics.calls = self.statistics.calls.saturating_add(1);
        let binding = self.lookup(&environment, name, span)?;
        match binding.value {
            BindingValue::Builtin(name) => self.call_builtin(name, &arguments, span),
            BindingValue::Function(function) => {
                if arguments.len() != function.parameters.len() {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::Arity,
                        span,
                        format!(
                            "function {:?} expects {} arguments but received {}",
                            function.name,
                            function.parameters.len(),
                            arguments.len()
                        ),
                    ));
                }
                if self.call_stack.len() >= self.limits.maximum_call_depth {
                    return Err(RuntimeFault::new(
                        RuntimeDiagnosticCode::CallDepth,
                        span,
                        format!(
                            "call depth exceeded the declared limit of {}",
                            self.limits.maximum_call_depth
                        ),
                    ));
                }
                let closure = function.closure.upgrade().ok_or_else(|| {
                    RuntimeFault::new(
                        RuntimeDiagnosticCode::InternalState,
                        span,
                        format!(
                            "lexical closure for {:?} is no longer available",
                            function.name
                        ),
                    )
                })?;
                let call_environment = Environment::child(closure);
                for (parameter, value) in function.parameters.iter().zip(arguments) {
                    self.declare(
                        &call_environment,
                        parameter,
                        Binding {
                            mutable: false,
                            value: BindingValue::Data(value),
                        },
                        span,
                    )?;
                }
                self.call_stack.push(CallSite {
                    function: function.name.clone(),
                    source_name: self.source_name.clone(),
                    span,
                });
                let previous_source =
                    std::mem::replace(&mut self.source_name, function.source_name.clone());
                let result = self.execute_statements(&function.body, call_environment, true);
                let result = match result {
                    Ok(flow) => Ok(flow),
                    Err(mut fault) => {
                        if fault.call_stack.is_empty() {
                            fault.call_stack = self.call_stack.clone();
                        }
                        Err(fault.at_source(function.source_name.clone()))
                    }
                };
                self.source_name = previous_source;
                self.call_stack.pop();
                match result? {
                    Flow::Continue(_) => Ok(Value::Null),
                    Flow::Return(value) => Ok(value),
                }
            }
            BindingValue::Data(value) => Err(RuntimeFault::new(
                RuntimeDiagnosticCode::NotCallable,
                span,
                format!("{} value {name:?} is not callable", value.type_name()),
            )),
        }
    }
}
