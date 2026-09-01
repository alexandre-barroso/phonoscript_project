use serde::{Deserialize, Serialize};

use crate::exact::NumericScalar;
use crate::phonology::StructuredCandidate;

pub const DOCUMENT_FORMAT: &str = "convalgen-analysis";
pub const DOCUMENT_VERSION: u32 = 4;
/// Reserved in-memory/storage marker for a violation cell the phonologist has
/// not yet supplied. It is never a mathematical violation count.
pub const UNSET_VIOLATION: u16 = u16::MAX;
pub const MAX_VIOLATION: u16 = UNSET_VIOLATION - 1;

/// Allocate a deterministic monotone project-local identity without coupling
/// it to an editable display label.
pub fn next_stable_id<'a>(prefix: &str, existing: impl IntoIterator<Item = &'a str>) -> String {
    let existing: std::collections::HashSet<&str> = existing.into_iter().collect();
    let mut suffix = existing
        .iter()
        .filter_map(|id| {
            id.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('-'))
                .and_then(|rest| rest.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    loop {
        let candidate = format!("{prefix}-{suffix}");
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvaluatorKind {
    Ot,
    HarmonicGrammar,
    MaxEnt,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TiePolicy {
    #[default]
    RetainAll,
    FirstListed,
    RequireUnique,
}

impl TiePolicy {
    pub const ALL: [Self; 3] = [Self::RetainAll, Self::FirstListed, Self::RequireUnique];

    pub const fn label(self) -> &'static str {
        match self {
            Self::RetainAll => "Retain all co-winners",
            Self::FirstListed => "Choose first listed",
            Self::RequireUnique => "Require a unique winner",
        }
    }

    pub const fn storage_value(self) -> &'static str {
        match self {
            Self::RetainAll => "retain all co-winners",
            Self::FirstListed => "choose first listed candidate",
            Self::RequireUnique => "require unique winner",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        Self::try_from_storage(value).unwrap_or_default()
    }

    pub fn try_from_storage(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "choose first listed" | "choose first listed candidate" | "first listed" => {
                Some(Self::FirstListed)
            }
            "require a unique winner" | "require unique winner" | "unique" => {
                Some(Self::RequireUnique)
            }
            "retain all co-winners" | "retain" | "all" => Some(Self::RetainAll),
            _ => None,
        }
    }
}

impl EvaluatorKind {
    pub const ALL: [Self; 3] = [Self::Ot, Self::HarmonicGrammar, Self::MaxEnt];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ot => "Optimality Theory",
            Self::HarmonicGrammar => "Harmonic Grammar",
            Self::MaxEnt => "Maximum Entropy",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Ot => "OT",
            Self::HarmonicGrammar => "HG",
            Self::MaxEnt => "MaxEnt",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryKind {
    WinnerSet,
    SurfaceWinnerSet,
    CompleteOrder,
    ProbabilityLaw,
    CandidateSupport,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComparisonMode {
    #[default]
    Exact,
    Approximate,
    Grid,
}

impl ComparisonMode {
    pub const ALL: [Self; 3] = [Self::Exact, Self::Approximate, Self::Grid];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Exact => "Exact",
            Self::Approximate => "Approximate",
            Self::Grid => "Grid-based",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponseDomain {
    #[default]
    Terminal,
    Trajectory,
}

impl ResponseDomain {
    pub const ALL: [Self; 2] = [Self::Terminal, Self::Trajectory];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal result",
            Self::Trajectory => "Complete trajectory",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NormalizerPolicy {
    #[default]
    Independent,
    SharedDeclared,
}

impl NormalizerPolicy {
    pub const ALL: [Self; 2] = [Self::Independent, Self::SharedDeclared];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Independent => "Independent normalizers",
            Self::SharedDeclared => "Shared normalizer (declared)",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConsumerMode {
    #[default]
    Direct,
    LaterConsumer,
}

impl ConsumerMode {
    pub const ALL: [Self; 2] = [Self::Direct, Self::LaterConsumer];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "Direct preservation",
            Self::LaterConsumer => "Under a later consumer",
        }
    }
}

impl QueryKind {
    pub const ALL: [Self; 5] = [
        Self::WinnerSet,
        Self::SurfaceWinnerSet,
        Self::CompleteOrder,
        Self::ProbabilityLaw,
        Self::CandidateSupport,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::WinnerSet => "winner set",
            Self::SurfaceWinnerSet => "surface winner set",
            Self::CompleteOrder => "complete candidate order",
            Self::ProbabilityLaw => "candidate probability law",
            Self::CandidateSupport => "candidate support",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecondOrderLayout {
    Overlay,
    DeltaSidecar,
    ExpandedPaired,
}

impl SecondOrderLayout {
    pub const ALL: [Self; 3] = [Self::Overlay, Self::DeltaSidecar, Self::ExpandedPaired];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Overlay => "Overlay",
            Self::DeltaSidecar => "Delta sidecar",
            Self::ExpandedPaired => "Expanded paired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlotKind {
    CandidateScores,
    CandidateProbabilities,
    ConstraintWeights,
    SerialPath,
    RankingShares,
}

impl PlotKind {
    pub const ALL: [Self; 5] = [
        Self::CandidateScores,
        Self::CandidateProbabilities,
        Self::ConstraintWeights,
        Self::SerialPath,
        Self::RankingShares,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::CandidateScores => "Candidate scores",
            Self::CandidateProbabilities => "Candidate probabilities",
            Self::ConstraintWeights => "Constraint weights",
            Self::SerialPath => "Serial path",
            Self::RankingShares => "Ranking shares",
        }
    }

    pub const fn label_for(self, evaluator: EvaluatorKind) -> &'static str {
        match (self, evaluator) {
            (Self::CandidateScores, EvaluatorKind::Ot) => "Candidate rank tiers",
            (Self::CandidateScores, _) => "Candidate costs",
            _ => self.label(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    /// Stable project-local identity, independent of the editable name.
    pub id: String,
    pub name: String,
    /// A fitted or analyst-declared weight. `None` records genuine absence;
    /// it is never interpreted as zero. A persisted [`MissingDependency`]
    /// explains why the parameter is unavailable and which operation it blocks.
    pub weight: Option<NumericScalar>,
    /// Lower values are higher strata. Constraints in one stratum are tied.
    pub stratum: usize,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub definition: String,
    #[serde(default)]
    pub prior_mean: NumericScalar,
    #[serde(default = "default_prior_sigma")]
    pub prior_sigma: NumericScalar,
}

const fn enabled() -> bool {
    true
}

fn default_prior_sigma() -> NumericScalar {
    NumericScalar::integer(100_000)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable tableau-local identity, independent of the editable label/form.
    pub id: String,
    pub name: String,
    /// Surface form represented by this candidate. It becomes an output only
    /// when the evaluator selects the candidate as a winner.
    pub form: String,
    pub violations: Vec<u16>,
    #[serde(default = "unit_mass")]
    pub base_mass: NumericScalar,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub observed_frequency: NumericScalar,
    /// Optional typed phonological structure. The flat `form` remains the
    /// display/search projection; this field preserves segmental, prosodic,
    /// and correspondence structure across `.ottab` round trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<StructuredCandidate>,
}

fn unit_mass() -> NumericScalar {
    NumericScalar::integer(1)
}

/// Contract stage at which an unavailable dependency prevents an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyStage {
    Formation,
    Admission,
}

/// Operation family for which a persisted dependency is required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DependencyScope {
    AnyEvaluation,
    Evaluator { evaluator: EvaluatorKind },
    Learning,
    ExactCertification,
}

/// A persisted, deliberately unsatisfied analysis dependency.
///
/// This is distinct from a zero, an empty string, and a disabled constraint.
/// It lets source-faithful ledgers preserve marks whose fitted parameters were
/// never published while guaranteeing that the checked engine refuses the
/// affected calculation at a stable coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingDependency {
    pub code: String,
    pub stage: DependencyStage,
    pub coordinate: String,
    pub scope: DependencyScope,
    pub message: String,
    pub remedy: String,
}

impl MissingDependency {
    pub fn blocks_evaluator(&self, evaluator: EvaluatorKind) -> bool {
        match self.scope {
            DependencyScope::AnyEvaluation => true,
            DependencyScope::Evaluator {
                evaluator: required,
            } => required == evaluator,
            DependencyScope::Learning | DependencyScope::ExactCertification => false,
        }
    }

    pub fn blocks_learning(&self) -> bool {
        matches!(self.scope, DependencyScope::Learning)
    }

    pub fn blocks_exact_certification(&self) -> bool {
        matches!(self.scope, DependencyScope::ExactCertification)
    }

    pub fn validate(&self) -> Result<(), String> {
        for (label, value) in [
            ("code", self.code.as_str()),
            ("coordinate", self.coordinate.as_str()),
            ("message", self.message.as_str()),
            ("remedy", self.remedy.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("missing dependency has an empty {label}"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tableau {
    /// Stable project-local identity, independent of the editable name/input.
    pub id: String,
    /// Human-facing name inside a multi-tableau project. Older `.ottab`
    /// documents omit it and fall back to the input or ordinal in the UI.
    #[serde(default)]
    pub name: String,
    pub input: String,
    pub constraints: Vec<Constraint>,
    pub candidates: Vec<Candidate>,
    #[serde(default = "default_tie_policy")]
    pub tie_policy: String,
    #[serde(default)]
    pub notes: String,
    /// Optional per-tableau evaluator for mixed-model research projects.
    /// When absent, the project evaluator remains the default.
    #[serde(default)]
    pub evaluator: Option<EvaluatorKind>,
    /// Optional per-tableau MaxEnt temperature.
    #[serde(default)]
    pub temperature: Option<NumericScalar>,
    /// Declared dependencies that are absent from this tableau's evidence.
    /// The document remains loadable, but checked operations in the named
    /// scope return a structured refusal rather than using placeholders.
    #[serde(default)]
    pub missing_dependencies: Vec<MissingDependency>,
    /// Regression expectations are human-readable project evidence, not an
    /// alternative evaluator. ConvalGEN always calculates the answer first.
    #[serde(default)]
    pub expected_winners: Vec<String>,
    #[serde(default)]
    pub source_locator: String,
}

fn default_tie_policy() -> String {
    "retain all co-winners".to_owned()
}

impl Tableau {
    pub fn evaluator_or(&self, default: EvaluatorKind) -> EvaluatorKind {
        self.evaluator.unwrap_or(default)
    }

    pub fn temperature_scalar_or<'a>(&'a self, default: &'a NumericScalar) -> &'a NumericScalar {
        self.temperature.as_ref().unwrap_or(default)
    }

    pub fn temperature_or(&self, default: &NumericScalar) -> f64 {
        self.temperature_scalar_or(default)
            .to_f64_center()
            .expect("a checked document has a finite representable temperature")
    }

    pub fn tie_policy_kind(&self) -> TiePolicy {
        TiePolicy::from_storage(&self.tie_policy)
    }

    pub fn set_tie_policy(&mut self, policy: TiePolicy) {
        self.tie_policy = policy.storage_value().to_owned();
    }

    pub fn normalize(&mut self) {
        // Normalization is deliberately presentation-only. Mathematical
        // defects must reach the checked engine as explicit refusals rather
        // than being silently repaired while loading or saving a project.
        self.tie_policy = self.tie_policy_kind().storage_value().to_owned();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerialMove {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub operation: String,
    pub violations: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerialSettings {
    pub start: String,
    pub moves: Vec<SerialMove>,
    pub maximum_steps: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecondOrderSettings {
    pub query: QueryKind,
    pub answer_sort: String,
    pub scope: String,
    pub transformation: String,
    pub transport: String,
    pub layout: SecondOrderLayout,
    #[serde(default)]
    pub comparison_mode: ComparisonMode,
    #[serde(default = "default_tolerance")]
    pub tolerance: NumericScalar,
    #[serde(default = "default_grid_step")]
    pub grid_step: NumericScalar,
    #[serde(default)]
    pub response_domain: ResponseDomain,
    #[serde(default)]
    pub normalizer_policy: NormalizerPolicy,
    #[serde(default = "default_scientific_layer")]
    pub source_layer: String,
    #[serde(default = "default_scientific_layer")]
    pub target_layer: String,
    #[serde(default = "default_layer_transport")]
    pub layer_transport: String,
    #[serde(default)]
    pub consumer_mode: ConsumerMode,
    #[serde(default)]
    pub consumer: String,
}

fn default_tolerance() -> NumericScalar {
    NumericScalar::parse_exact("0.000000001").expect("static exact tolerance")
}

fn default_grid_step() -> NumericScalar {
    NumericScalar::parse_exact("0.001").expect("static exact grid step")
}

fn default_scientific_layer() -> String {
    "grammar".to_owned()
}

fn default_layer_transport() -> String {
    "identity".to_owned()
}

fn default_serial_settings() -> SerialSettings {
    SerialSettings {
        start: String::new(),
        moves: Vec::new(),
        maximum_steps: 64,
    }
}

impl Default for SecondOrderSettings {
    fn default() -> Self {
        Self {
            query: QueryKind::WinnerSet,
            answer_sort: "set of candidate identities".to_owned(),
            scope: "complete registered candidate support".to_owned(),
            transformation: "identity".to_owned(),
            transport: "identity on candidate identities".to_owned(),
            layout: SecondOrderLayout::Overlay,
            comparison_mode: ComparisonMode::Exact,
            tolerance: default_tolerance(),
            grid_step: default_grid_step(),
            response_domain: ResponseDomain::Terminal,
            normalizer_policy: NormalizerPolicy::Independent,
            source_layer: default_scientific_layer(),
            target_layer: default_scientific_layer(),
            layer_transport: default_layer_transport(),
            consumer_mode: ConsumerMode::Direct,
            consumer: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresentationSettings {
    #[serde(default = "enabled")]
    pub compact_rows: bool,
    #[serde(default = "enabled")]
    pub show_title: bool,
    #[serde(default)]
    pub show_author: bool,
    #[serde(default)]
    pub show_legend: bool,
    #[serde(default = "default_export_scale")]
    pub export_scale: f32,
}

const fn default_export_scale() -> f32 {
    1.0
}

impl Default for PresentationSettings {
    fn default() -> Self {
        Self {
            compact_rows: true,
            show_title: true,
            show_author: false,
            show_legend: false,
            export_scale: default_export_scale(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvalgenDocument {
    /// Stable document identity, independent of title and filesystem path.
    pub id: String,
    pub format: String,
    pub format_version: u32,
    pub application: String,
    pub title: String,
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    pub evaluator: EvaluatorKind,
    pub temperature: NumericScalar,
    pub source: Tableau,
    pub target: Tableau,
    #[serde(default)]
    pub dataset: Vec<Tableau>,
    #[serde(default)]
    pub a_priori_rankings: Vec<(usize, usize)>,
    pub serial: SerialSettings,
    #[serde(default = "default_serial_settings")]
    pub target_serial: SerialSettings,
    pub second_order: SecondOrderSettings,
    pub clone_constraint: usize,
    pub plot: PlotKind,
    #[serde(default)]
    pub presentation: PresentationSettings,
}

impl ConvalgenDocument {
    pub fn blank() -> Self {
        let tableau = Tableau {
            id: "tableau-1".to_owned(),
            name: "Tableau 1".to_owned(),
            input: String::new(),
            constraints: vec![Constraint {
                id: "constraint-1".to_owned(),
                name: "C1".to_owned(),
                weight: Some(NumericScalar::integer(1)),
                stratum: 0,
                enabled: true,
                definition: String::new(),
                prior_mean: NumericScalar::integer(0),
                prior_sigma: default_prior_sigma(),
            }],
            candidates: vec![Candidate {
                id: "candidate-1".to_owned(),
                name: "candidate 1".to_owned(),
                form: "candidate 1".to_owned(),
                violations: vec![UNSET_VIOLATION],
                base_mass: NumericScalar::integer(1),
                notes: String::new(),
                observed_frequency: NumericScalar::integer(1),
                structured: None,
            }],
            tie_policy: default_tie_policy(),
            notes: String::new(),
            evaluator: None,
            temperature: None,
            missing_dependencies: Vec::new(),
            expected_winners: Vec::new(),
            source_locator: String::new(),
        };
        Self {
            id: "project-1".to_owned(),
            format: DOCUMENT_FORMAT.to_owned(),
            format_version: DOCUMENT_VERSION,
            application: "ConvalGEN".to_owned(),
            title: "Untitled Analysis".to_owned(),
            author: String::new(),
            description: String::new(),
            keywords: Vec::new(),
            evaluator: EvaluatorKind::Ot,
            temperature: NumericScalar::integer(1),
            target: tableau.clone(),
            dataset: vec![tableau.clone()],
            source: tableau,
            a_priori_rankings: Vec::new(),
            serial: default_serial_settings(),
            target_serial: default_serial_settings(),
            second_order: SecondOrderSettings::default(),
            clone_constraint: 0,
            plot: PlotKind::CandidateScores,
            presentation: PresentationSettings::default(),
        }
    }

    pub fn normalize(&mut self) {
        self.format = DOCUMENT_FORMAT.to_owned();
        self.format_version = DOCUMENT_VERSION;
        self.application = "ConvalGEN".to_owned();
        self.source.normalize();
        self.target.normalize();
        for tableau in &mut self.dataset {
            tableau.normalize();
        }
        self.presentation.export_scale = if self.presentation.export_scale.is_finite() {
            self.presentation.export_scale.clamp(0.5, 4.0)
        } else {
            1.0
        };
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != DOCUMENT_FORMAT {
            return Err("not a ConvalGEN analysis document".to_owned());
        }
        if self.format_version != DOCUMENT_VERSION {
            return Err(format!(
                "unsupported .ottab format version {} (this build reads version {})",
                self.format_version, DOCUMENT_VERSION
            ));
        }
        if self.id.trim().is_empty() {
            return Err("project id must be nonempty".to_owned());
        }
        validate_positive_scalar(&self.temperature, "project temperature")?;
        if self.serial.maximum_steps == 0 || self.target_serial.maximum_steps == 0 {
            return Err("serial maximum-steps must be strictly positive".to_owned());
        }
        validate_nonnegative_scalar(&self.second_order.tolerance, "second-order tolerance")?;
        validate_positive_scalar(&self.second_order.grid_step, "second-order grid step")?;
        for (label, tableau) in [("source", &self.source), ("target", &self.target)] {
            if tableau.constraints.is_empty() {
                return Err(format!("{label} tableau has no constraints"));
            }
            if tableau.candidates.is_empty() {
                return Err(format!("{label} tableau has no candidates"));
            }
            if tableau
                .candidates
                .iter()
                .any(|candidate| candidate.violations.len() != tableau.constraints.len())
            {
                return Err(format!(
                    "{label} tableau has a nonrectangular violation matrix"
                ));
            }
            validate_tableau_numbers(tableau)
                .map_err(|problem| format!("{label} tableau {problem}"))?;
        }
        for (index, tableau) in self.dataset.iter().enumerate() {
            if tableau.constraints.is_empty() || tableau.candidates.is_empty() {
                return Err(format!("dataset tableau {} is empty", index + 1));
            }
            if tableau
                .candidates
                .iter()
                .any(|candidate| candidate.violations.len() != tableau.constraints.len())
            {
                return Err(format!("dataset tableau {} is nonrectangular", index + 1));
            }
            validate_tableau_numbers(tableau)
                .map_err(|problem| format!("dataset tableau {} {problem}", index + 1))?;
        }
        let mut tableau_ids = std::collections::HashSet::new();
        for (index, tableau) in self.dataset.iter().enumerate() {
            if !tableau_ids.insert(tableau.id.as_str()) {
                return Err(format!(
                    "dataset tableau {} duplicates stable id `{}`",
                    index + 1,
                    tableau.id
                ));
            }
        }
        if self.clone_constraint >= self.source.constraints.len() {
            return Err("Q-Calculus clone constraint is outside the source register".to_owned());
        }
        Ok(())
    }
}

fn validate_tableau_numbers(tableau: &Tableau) -> Result<(), String> {
    if tableau.id.trim().is_empty() {
        return Err("has an empty stable tableau id".to_owned());
    }
    if TiePolicy::try_from_storage(&tableau.tie_policy).is_none() {
        return Err("has an unknown tie policy".to_owned());
    }
    if let Some(temperature) = &tableau.temperature {
        validate_positive_scalar(temperature, "tableau temperature")?;
    }
    for dependency in &tableau.missing_dependencies {
        dependency.validate()?;
    }
    for (index, constraint) in tableau.constraints.iter().enumerate() {
        if constraint.id.trim().is_empty() {
            return Err(format!("constraint[{index}] has an empty stable id"));
        }
        if constraint.definition.trim_start().starts_with("calc:") {
            return Err(format!(
                "constraint[{index}] uses a retired calculated-mark declaration; replace it with descriptive prose and enter every violation count explicitly"
            ));
        }
        if let Some(weight) = &constraint.weight {
            scalar_center(weight, &format!("constraint[{index}] weight"))?;
        }
        scalar_center(
            &constraint.prior_mean,
            &format!("constraint[{index}] prior mean"),
        )?;
        validate_positive_scalar(
            &constraint.prior_sigma,
            &format!("constraint[{index}] prior scale"),
        )?;
    }
    for (index, candidate) in tableau.candidates.iter().enumerate() {
        if candidate.id.trim().is_empty() {
            return Err(format!("candidate[{index}] has an empty stable id"));
        }
        validate_positive_scalar(
            &candidate.base_mass,
            &format!("candidate[{index}] base mass"),
        )?;
        validate_nonnegative_scalar(
            &candidate.observed_frequency,
            &format!("candidate[{index}] observed frequency"),
        )?;
    }
    let constraint_ids: std::collections::HashSet<&str> = tableau
        .constraints
        .iter()
        .map(|constraint| constraint.id.as_str())
        .collect();
    if constraint_ids.len() != tableau.constraints.len() {
        return Err("has duplicate stable constraint ids".to_owned());
    }
    let candidate_ids: std::collections::HashSet<&str> = tableau
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    if candidate_ids.len() != tableau.candidates.len() {
        return Err("has duplicate stable candidate ids".to_owned());
    }
    Ok(())
}

pub fn scalar_center(value: &NumericScalar, coordinate: &str) -> Result<f64, String> {
    value
        .to_f64_center()
        .map_err(|problem| format!("{coordinate} cannot be evaluated numerically: {problem}"))
}

pub fn validate_positive_scalar(value: &NumericScalar, coordinate: &str) -> Result<(), String> {
    if scalar_center(value, coordinate)? <= 0.0 {
        return Err(format!("{coordinate} must be strictly positive"));
    }
    Ok(())
}

pub fn validate_nonnegative_scalar(value: &NumericScalar, coordinate: &str) -> Result<(), String> {
    if scalar_center(value, coordinate)? < 0.0 {
        return Err(format!("{coordinate} must be nonnegative"));
    }
    Ok(())
}
