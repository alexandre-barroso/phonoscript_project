//! One checked entry point for every analytical operation used by ConvalGEN
//! and PhonoScript.
//!
//! The lower-level modules remain deliberately small and fast. This facade is
//! the trust boundary: it performs formation checks once, assigns stable error
//! codes, and then dispatches the same operation for the GUI, command-line
//! interpreter, and embeddable Rust API.

use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::engine::{
    CloneAuditResult, ComparisonRefusal, ComparisonStatus, ContractStage, EvaluationContext,
    RankingSpaceBudget, RankingSpaceError, RankingSpaceErrorKind, RankingSpaceResult,
    SecondOrderResult, SerialResult, TableauEvaluation, clone_audit_with_budget,
    compare_tableaux_with_contexts, evaluate, evaluate_serial, ranking_space_with_budget,
};
use crate::learning::{
    HarmonicBound, MaxEntTrainingResult, RankingInference, harmonic_bounds,
    individually_unnecessary_constraints, infer_ot_ranking, train_maxent,
};
use crate::model::{
    ConvalgenDocument, DependencyStage, EvaluatorKind, SerialSettings, Tableau, TiePolicy,
    UNSET_VIOLATION, scalar_center,
};
use crate::ranking::{
    ConstraintDemotionResult, LinearExtensions, MarkData, PartialRanking, constraint_demotion,
    mark_data,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineStage {
    Formation,
    Admission,
    Evaluation,
    Learning,
    Search,
}

impl Display for EngineStage {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Formation => "formation",
            Self::Admission => "admission",
            Self::Evaluation => "evaluation",
            Self::Learning => "learning",
            Self::Search => "search",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    pub code: String,
    pub stage: EngineStage,
    pub coordinate: String,
    pub message: String,
    pub remedy: String,
}

impl EngineError {
    fn new(
        code: &str,
        stage: EngineStage,
        coordinate: impl Into<String>,
        message: impl Into<String>,
        remedy: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_owned(),
            stage,
            coordinate: coordinate.into(),
            message: message.into(),
            remedy: remedy.into(),
        }
    }

    fn lower_level(code: &str, stage: EngineStage, coordinate: &str, message: String) -> Self {
        Self::new(
            code,
            stage,
            coordinate,
            message,
            "Inspect the named analysis coordinate and supply a well-formed declaration.",
        )
    }
}

impl Display for EngineError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} [{}:{}] {} Remedy: {}",
            self.code, self.stage, self.coordinate, self.message, self.remedy
        )
    }
}

impl Error for EngineError {}

pub type EngineResult<T> = Result<T, EngineError>;

#[derive(Debug, Clone, Copy, Default)]
pub struct PhonologicalEngine;

impl PhonologicalEngine {
    pub const fn new() -> Self {
        Self
    }

    pub fn mark_data(
        &self,
        tableau: &Tableau,
        winner: usize,
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<MarkData> {
        self.validate_tableau(tableau, evaluator, temperature)?;
        mark_data(tableau, winner).map_err(|message| {
            EngineError::lower_level(
                "PE-LEARN-MARK-DATA",
                EngineStage::Learning,
                "learning.mark-data",
                message,
            )
        })
    }

    pub fn constraint_demotion(&self, data: &MarkData) -> ConstraintDemotionResult {
        constraint_demotion(data)
    }

    pub fn linear_extensions(
        &self,
        partial_ranking: &PartialRanking,
        limit: usize,
    ) -> LinearExtensions {
        partial_ranking.linear_extensions(limit)
    }

    /// Check the finite first-order analysis without silently normalising or
    /// repairing it. Violation counts come only from the stored analyst ledger.
    pub fn validate_tableau(
        &self,
        tableau: &Tableau,
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<()> {
        if let Some(dependency) = tableau
            .missing_dependencies
            .iter()
            .find(|dependency| dependency.blocks_evaluator(evaluator))
        {
            return Err(EngineError::new(
                &dependency.code,
                match dependency.stage {
                    DependencyStage::Formation => EngineStage::Formation,
                    DependencyStage::Admission => EngineStage::Admission,
                },
                dependency.coordinate.clone(),
                dependency.message.clone(),
                dependency.remedy.clone(),
            ));
        }
        if tableau.id.trim().is_empty() {
            return Err(EngineError::new(
                "PE-FORM-TABLEAU-ID",
                EngineStage::Formation,
                "tableau.id",
                "the tableau has an empty stable identity",
                "Assign a non-display stable identity before evaluation.",
            ));
        }
        if tableau.constraints.is_empty() {
            return Err(EngineError::new(
                "PE-FORM-CONSTRAINTS",
                EngineStage::Formation,
                "tableau.constraints",
                "the tableau has no constraints",
                "Declare at least one constraint before evaluation.",
            ));
        }
        if tableau.candidates.is_empty() {
            return Err(EngineError::new(
                "PE-FORM-CANDIDATES",
                EngineStage::Formation,
                "tableau.candidates",
                "the tableau has no candidates",
                "Declare the finite candidate support before evaluation.",
            ));
        }
        if TiePolicy::try_from_storage(&tableau.tie_policy).is_none() {
            return Err(EngineError::new(
                "PE-FORM-TIE-POLICY",
                EngineStage::Formation,
                "tableau.tie-policy",
                format!("unknown tie policy `{}`", tableau.tie_policy),
                "Choose retain-all, first-listed, or require-unique explicitly.",
            ));
        }
        if !temperature.is_finite() || temperature <= 0.0 {
            return Err(EngineError::new(
                "PE-ADMIT-TEMPERATURE",
                EngineStage::Admission,
                "evaluator.temperature",
                "temperature must be finite and strictly positive",
                "Supply a positive finite temperature; use 1 for the conventional scale.",
            ));
        }

        let mut constraint_ids = HashSet::new();
        let mut constraint_names = HashSet::new();
        for (index, constraint) in tableau.constraints.iter().enumerate() {
            let coordinate = format!("constraint[{index}]");
            if constraint.id.trim().is_empty() || !constraint_ids.insert(constraint.id.as_str()) {
                return Err(EngineError::new(
                    "PE-FORM-CONSTRAINT-ID",
                    EngineStage::Formation,
                    format!("{coordinate}.id"),
                    "constraint stable identities must be nonempty and unique within a tableau",
                    "Assign every constraint a distinct non-display identity.",
                ));
            }
            if constraint.name.trim().is_empty() {
                return Err(EngineError::new(
                    "PE-FORM-CONSTRAINT-NAME",
                    EngineStage::Formation,
                    coordinate,
                    "a constraint has an empty name",
                    "Give every constraint a stable, nonempty identity.",
                ));
            }
            if !constraint_names.insert(constraint.name.as_str()) {
                return Err(EngineError::new(
                    "PE-FORM-CONSTRAINT-DUPLICATE",
                    EngineStage::Formation,
                    coordinate,
                    format!("constraint name `{}` is duplicated", constraint.name),
                    "Use one unique name for each constraint in a tableau.",
                ));
            }
            let weighted = matches!(
                evaluator,
                EvaluatorKind::HarmonicGrammar | EvaluatorKind::MaxEnt
            ) && constraint.enabled;
            let weight = match &constraint.weight {
                Some(weight) => Some(scalar_center(weight, &format!("{coordinate}.weight"))),
                None if weighted => {
                    return Err(EngineError::new(
                        "PE-ADMIT-MISSING-WEIGHT",
                        EngineStage::Admission,
                        format!("{coordinate}.weight"),
                        format!("weight for `{}` is unavailable", constraint.name),
                        "Supply the fitted weight or retain the tableau as a nonevaluable mark ledger.",
                    ));
                }
                None => None,
            };
            let prior_is_valid = evaluator != EvaluatorKind::MaxEnt
                || (scalar_center(&constraint.prior_mean, &format!("{coordinate}.prior-mean"))
                    .is_ok()
                    && scalar_center(
                        &constraint.prior_sigma,
                        &format!("{coordinate}.prior-sigma"),
                    )
                    .is_ok_and(|value| value > 0.0));
            if weight.as_ref().is_some_and(Result::is_err) || !prior_is_valid {
                return Err(EngineError::new(
                    "PE-FORM-CONSTRAINT-NUMERIC",
                    EngineStage::Formation,
                    coordinate,
                    "constraint weight or prior is nonfinite, or the prior scale is nonpositive",
                    "Use finite weights and prior means with a strictly positive finite prior scale.",
                ));
            }
            if weighted
                && weight
                    .and_then(Result::ok)
                    .is_some_and(|weight| weight < 0.0)
            {
                return Err(EngineError::new(
                    "PE-ADMIT-NEGATIVE-WEIGHT",
                    EngineStage::Admission,
                    coordinate,
                    "enabled HG and MaxEnt constraints require nonnegative weights",
                    "Use a nonnegative cost weight or explicitly disable the constraint.",
                ));
            }
        }

        let mut candidate_ids = HashSet::new();
        let mut candidate_names = HashSet::new();
        for (candidate_index, candidate) in tableau.candidates.iter().enumerate() {
            let coordinate = format!("candidate[{candidate_index}]");
            if candidate.id.trim().is_empty() || !candidate_ids.insert(candidate.id.as_str()) {
                return Err(EngineError::new(
                    "PE-FORM-CANDIDATE-ID",
                    EngineStage::Formation,
                    format!("{coordinate}.id"),
                    "candidate stable identities must be nonempty and unique within a tableau",
                    "Assign every candidate a distinct non-display identity.",
                ));
            }
            if candidate.name.trim().is_empty() {
                return Err(EngineError::new(
                    "PE-FORM-CANDIDATE-NAME",
                    EngineStage::Formation,
                    coordinate,
                    "a candidate has an empty name",
                    "Give every candidate a stable, nonempty identity.",
                ));
            }
            if !candidate_names.insert(candidate.name.as_str()) {
                return Err(EngineError::new(
                    "PE-FORM-CANDIDATE-DUPLICATE",
                    EngineStage::Formation,
                    coordinate,
                    format!("candidate name `{}` is duplicated", candidate.name),
                    "Use one unique candidate identity within each tableau.",
                ));
            }
            if candidate.violations.len() != tableau.constraints.len() {
                return Err(EngineError::new(
                    "PE-FORM-MATRIX",
                    EngineStage::Formation,
                    coordinate,
                    format!(
                        "candidate `{}` has {} marks for {} constraints",
                        candidate.name,
                        candidate.violations.len(),
                        tableau.constraints.len()
                    ),
                    "Supply exactly one violation value per declared constraint.",
                ));
            }
            if let Some(constraint_index) = candidate
                .violations
                .iter()
                .position(|mark| *mark == UNSET_VIOLATION)
            {
                return Err(EngineError::new(
                    "PE-FORM-VIOLATION-UNSET",
                    EngineStage::Formation,
                    format!("{coordinate}.violations[{constraint_index}]"),
                    "the violation count has not been supplied by the phonologist",
                    "Enter a nonnegative violation count in this cell before evaluation.",
                ));
            }
            if !scalar_center(&candidate.base_mass, &format!("{coordinate}.base-mass"))
                .is_ok_and(|value| value > 0.0)
            {
                return Err(EngineError::new(
                    "PE-FORM-BASE-MASS",
                    EngineStage::Formation,
                    coordinate.clone(),
                    "candidate base mass must be finite and strictly positive",
                    "Use mass 1 when no nonuniform base measure is intended.",
                ));
            }
            if !scalar_center(
                &candidate.observed_frequency,
                &format!("{coordinate}.observed-frequency"),
            )
            .is_ok_and(|value| value >= 0.0)
            {
                return Err(EngineError::new(
                    "PE-FORM-OBSERVATION",
                    EngineStage::Formation,
                    coordinate,
                    "observed frequency must be finite and nonnegative",
                    "Use a finite count or mass at least zero.",
                ));
            }
        }
        Ok(())
    }

    pub fn evaluate(
        &self,
        tableau: &Tableau,
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<TableauEvaluation> {
        self.validate_tableau(tableau, evaluator, temperature)?;
        Ok(evaluate(tableau, evaluator, temperature))
    }

    pub fn evaluate_in_project(
        &self,
        project: &ConvalgenDocument,
        tableau: &Tableau,
    ) -> EngineResult<TableauEvaluation> {
        self.evaluate(
            tableau,
            tableau.evaluator_or(project.evaluator),
            tableau.temperature_or(&project.temperature),
        )
    }

    pub fn validate_serial(
        &self,
        tableau: &Tableau,
        serial: &SerialSettings,
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<()> {
        self.validate_tableau(tableau, evaluator, temperature)?;
        if serial.start.trim().is_empty() {
            return Err(EngineError::new(
                "PE-FORM-SERIAL-START",
                EngineStage::Formation,
                "serial.start",
                "the serial derivation has no initial form",
                "Declare the initial representation before the move ledger.",
            ));
        }
        if serial.maximum_steps == 0 {
            return Err(EngineError::new(
                "PE-FORM-SERIAL-LIMIT",
                EngineStage::Formation,
                "serial.maximum-steps",
                "the serial step limit is zero",
                "Declare a positive finite step limit.",
            ));
        }
        for (index, movement) in serial.moves.iter().enumerate() {
            if movement.from.trim().is_empty() || movement.to.trim().is_empty() {
                return Err(EngineError::new(
                    "PE-FORM-SERIAL-MOVE",
                    EngineStage::Formation,
                    format!("serial.move[{index}]"),
                    "a serial move has an empty source or candidate form",
                    "Declare both endpoints of every GEN1 move.",
                ));
            }
            if movement.violations.len() != tableau.constraints.len() {
                return Err(EngineError::new(
                    "PE-FORM-SERIAL-MATRIX",
                    EngineStage::Formation,
                    format!("serial.move[{index}].violations"),
                    "a serial move does not match the constraint register",
                    "Supply exactly one mark for every constraint on every move.",
                ));
            }
            if let Some(constraint_index) = movement
                .violations
                .iter()
                .position(|mark| *mark == UNSET_VIOLATION)
            {
                return Err(EngineError::new(
                    "PE-FORM-SERIAL-VIOLATION-UNSET",
                    EngineStage::Formation,
                    format!("serial.move[{index}].violations[{constraint_index}]"),
                    "the serial violation count has not been supplied by the phonologist",
                    "Enter a nonnegative violation count in this cell before serial evaluation.",
                ));
            }
        }
        Ok(())
    }

    pub fn serial(
        &self,
        tableau: &Tableau,
        serial: &SerialSettings,
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<SerialResult> {
        self.validate_serial(tableau, serial, evaluator, temperature)?;
        Ok(evaluate_serial(
            &tableau.constraints,
            &serial.moves,
            &serial.start,
            evaluator,
            temperature,
            serial.maximum_steps,
        ))
    }

    /// A comparison remains a value even when its first-order inputs are not
    /// formed. This preserves the dissertation's three-way result type.
    pub fn compare(&self, project: &ConvalgenDocument) -> SecondOrderResult {
        let source_context = EvaluationContext {
            kind: project.source.evaluator_or(project.evaluator),
            temperature: project.source.temperature_scalar_or(&project.temperature),
        };
        let target_context = EvaluationContext {
            kind: project.target.evaluator_or(project.evaluator),
            temperature: project.target.temperature_scalar_or(&project.temperature),
        };
        for (side, tableau, serial, context) in [
            ("source", &project.source, &project.serial, source_context),
            (
                "target",
                &project.target,
                &project.target_serial,
                target_context,
            ),
        ] {
            let validation = if project.second_order.response_domain
                == crate::model::ResponseDomain::Trajectory
            {
                self.validate_serial(
                    tableau,
                    serial,
                    context.kind,
                    context.temperature.to_f64_center().unwrap_or(f64::NAN),
                )
            } else {
                self.validate_tableau(
                    tableau,
                    context.kind,
                    context.temperature.to_f64_center().unwrap_or(f64::NAN),
                )
            };
            if let Err(problem) = validation {
                return SecondOrderResult {
                    status: ComparisonStatus::NotEvaluated,
                    source_answer: Vec::new(),
                    transported_source_answer: Vec::new(),
                    target_answer: Vec::new(),
                    discrepancies: Vec::new(),
                    refusal: Some(ComparisonRefusal {
                        code: problem.code,
                        stage: match problem.stage {
                            EngineStage::Formation => ContractStage::Formation,
                            EngineStage::Admission => ContractStage::Admission,
                            _ => ContractStage::Evaluation,
                        },
                        coordinate: format!("{side}.{}", problem.coordinate),
                        message: problem.message,
                        remedy: problem.remedy,
                    }),
                    certificate: None,
                    source_normalizer: None,
                    target_normalizer: None,
                };
            }
        }
        compare_tableaux_with_contexts(
            &project.source,
            &project.target,
            &project.serial,
            &project.target_serial,
            source_context,
            target_context,
            &project.second_order,
        )
    }

    fn validate_dataset(
        &self,
        tableaus: &[Tableau],
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<()> {
        if tableaus.is_empty() {
            return Err(EngineError::new(
                "PE-FORM-DATASET",
                EngineStage::Formation,
                "project.tableaux",
                "the project contains no tableaux",
                "Declare at least one tableau.",
            ));
        }
        for (index, tableau) in tableaus.iter().enumerate() {
            self.validate_tableau(
                tableau,
                tableau.evaluator_or(evaluator),
                tableau
                    .temperature
                    .as_ref()
                    .map(|value| value.to_f64_center().unwrap_or(f64::NAN))
                    .unwrap_or(temperature),
            )
            .map_err(|mut problem| {
                problem.coordinate = format!("tableau[{index}].{}", problem.coordinate);
                problem
            })?;
        }
        Ok(())
    }

    pub fn q_ranking_space(
        &self,
        tableaus: &[Tableau],
        a_priori_rankings: &[(usize, usize)],
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<RankingSpaceResult> {
        self.q_ranking_space_with_budget(
            tableaus,
            a_priori_rankings,
            evaluator,
            temperature,
            RankingSpaceBudget::default(),
        )
    }

    pub fn q_ranking_space_with_budget(
        &self,
        tableaus: &[Tableau],
        a_priori_rankings: &[(usize, usize)],
        evaluator: EvaluatorKind,
        temperature: f64,
        budget: RankingSpaceBudget,
    ) -> EngineResult<RankingSpaceResult> {
        self.validate_q_semantics(tableaus, a_priori_rankings, evaluator)?;
        self.validate_dataset(tableaus, evaluator, temperature)?;
        ranking_space_with_budget(tableaus, budget).map_err(|problem| {
            Self::q_search_error(problem, "PE-Q-RANKING-SPACE", "q-calculus.ranking-space")
        })
    }

    pub fn q_clone_audit(
        &self,
        tableau: &Tableau,
        constraint: usize,
        a_priori_rankings: &[(usize, usize)],
        evaluator: EvaluatorKind,
        temperature: f64,
    ) -> EngineResult<CloneAuditResult> {
        self.q_clone_audit_with_budget(
            tableau,
            constraint,
            a_priori_rankings,
            evaluator,
            temperature,
            RankingSpaceBudget::default(),
        )
    }

    pub fn q_clone_audit_with_budget(
        &self,
        tableau: &Tableau,
        constraint: usize,
        a_priori_rankings: &[(usize, usize)],
        evaluator: EvaluatorKind,
        temperature: f64,
        budget: RankingSpaceBudget,
    ) -> EngineResult<CloneAuditResult> {
        self.validate_q_semantics(std::slice::from_ref(tableau), a_priori_rankings, evaluator)?;
        self.validate_tableau(tableau, evaluator, temperature)?;
        clone_audit_with_budget(tableau, constraint, budget).map_err(|problem| {
            Self::q_search_error(problem, "PE-Q-CLONE-AUDIT", "q-calculus.clone")
        })
    }

    fn validate_q_semantics(
        &self,
        tableaus: &[Tableau],
        a_priori_rankings: &[(usize, usize)],
        evaluator: EvaluatorKind,
    ) -> EngineResult<()> {
        if !a_priori_rankings.is_empty() {
            return Err(EngineError::new(
                "PE-Q-A-PRIORI-UNSUPPORTED",
                EngineStage::Admission,
                "q-calculus.ranking-relation",
                "this Q-Calculus operation counts strict rankings compatible with tableau strata; it does not silently merge the project a-priori relation",
                "Clear the project a-priori relation for this audit or use OT ranking inference, which has separately defined a-priori semantics.",
            ));
        }
        for (index, tableau) in tableaus.iter().enumerate() {
            let effective = tableau.evaluator_or(evaluator);
            if effective != EvaluatorKind::Ot {
                return Err(EngineError::new(
                    "PE-Q-EVALUATOR",
                    EngineStage::Admission,
                    format!("tableau[{index}].evaluator"),
                    format!(
                        "Q ranking-space semantics are registered for strict OT, not {}",
                        effective.label()
                    ),
                    "Use an OT tableau or register a separate evaluator-specific Q operation before comparing HG or MaxEnt analyses.",
                ));
            }
            match tableau.tie_policy_kind() {
                TiePolicy::RetainAll => {}
                TiePolicy::FirstListed => {
                    return Err(EngineError::new(
                        "PE-Q-TIE-POLICY",
                        EngineStage::Admission,
                        format!("tableau[{index}].tie-policy"),
                        "Q ranking shares retain complete co-winner sets; first-listed row-order selection is not a registered ranking-world response",
                        "Set the tableau tie policy to retain all co-winners for this Q operation.",
                    ));
                }
                TiePolicy::RequireUnique => {
                    return Err(EngineError::new(
                        "PE-Q-UNIQUE-WINNER",
                        EngineStage::Admission,
                        format!("tableau[{index}].tie-policy"),
                        "the require-unique policy can leave tied ranking worlds unresolved, while this Q operation counts retained co-winner sets",
                        "Set the tableau tie policy to retain all co-winners or register an explicit unresolved-world response type.",
                    ));
                }
            }
        }
        Ok(())
    }

    fn q_search_error(
        problem: RankingSpaceError,
        invalid_code: &str,
        coordinate: &str,
    ) -> EngineError {
        let code = match problem.kind {
            RankingSpaceErrorKind::InvalidInput => invalid_code,
            RankingSpaceErrorKind::StateBudgetExceeded => "PE-Q-STATE-BUDGET",
        };
        EngineError::new(
            code,
            EngineStage::Search,
            coordinate,
            problem.message,
            match problem.kind {
                RankingSpaceErrorKind::InvalidInput => {
                    "Inspect the finite OT ledger and supply aligned candidates and enabled constraints."
                }
                RankingSpaceErrorKind::StateBudgetExceeded => {
                    "Increase the explicit Q state budget only after checking the intended finite support and ranking relation."
                }
            },
        )
    }

    pub fn learn_maxent(
        &self,
        tableaus: &[Tableau],
        temperature: f64,
        maximum_iterations: usize,
    ) -> EngineResult<MaxEntTrainingResult> {
        self.validate_learning_dependencies(tableaus)?;
        for (index, tableau) in tableaus.iter().enumerate() {
            let effective_evaluator = tableau.evaluator_or(EvaluatorKind::MaxEnt);
            if effective_evaluator != EvaluatorKind::MaxEnt {
                return Err(EngineError::new(
                    "PE-ADMIT-LEARNING-EVALUATOR",
                    EngineStage::Admission,
                    format!("tableau[{index}].evaluator"),
                    format!(
                        "MaxEnt learning cannot evaluate a tableau whose effective evaluator is {}",
                        effective_evaluator.short_label()
                    ),
                    "Remove the incompatible evaluator override or set this tableau to MaxEnt before learning.",
                ));
            }
        }
        self.validate_dataset(tableaus, EvaluatorKind::MaxEnt, temperature)?;
        train_maxent(tableaus, temperature, maximum_iterations).map_err(|message| {
            EngineError::lower_level(
                "PE-LEARN-MAXENT",
                EngineStage::Learning,
                "learning.maxent",
                message,
            )
        })
    }

    pub fn infer_ranking(
        &self,
        tableaus: &[Tableau],
        a_priori_rankings: &[(usize, usize)],
    ) -> EngineResult<RankingInference> {
        self.validate_learning_dependencies(tableaus)?;
        self.validate_dataset(tableaus, EvaluatorKind::Ot, 1.0)?;
        infer_ot_ranking(tableaus, a_priori_rankings).map_err(|message| {
            EngineError::lower_level(
                "PE-LEARN-OT",
                EngineStage::Learning,
                "learning.ranking",
                message,
            )
        })
    }

    pub fn harmonic_bounds(&self, tableaus: &[Tableau]) -> EngineResult<Vec<HarmonicBound>> {
        self.validate_dataset(tableaus, EvaluatorKind::Ot, 1.0)?;
        Ok(harmonic_bounds(tableaus))
    }

    pub fn unnecessary_constraints(
        &self,
        tableaus: &[Tableau],
        a_priori_rankings: &[(usize, usize)],
    ) -> EngineResult<Vec<usize>> {
        self.validate_dataset(tableaus, EvaluatorKind::Ot, 1.0)?;
        individually_unnecessary_constraints(tableaus, a_priori_rankings).map_err(|message| {
            EngineError::lower_level(
                "PE-DIAGNOSE-CONSTRAINTS",
                EngineStage::Search,
                "diagnostics.constraints",
                message,
            )
        })
    }

    fn validate_learning_dependencies(&self, tableaus: &[Tableau]) -> EngineResult<()> {
        for (tableau_index, tableau) in tableaus.iter().enumerate() {
            if let Some(dependency) = tableau
                .missing_dependencies
                .iter()
                .find(|dependency| dependency.blocks_learning())
            {
                return Err(EngineError::new(
                    &dependency.code,
                    match dependency.stage {
                        DependencyStage::Formation => EngineStage::Formation,
                        DependencyStage::Admission => EngineStage::Admission,
                    },
                    format!("tableau[{tableau_index}].{}", dependency.coordinate),
                    dependency.message.clone(),
                    dependency.remedy.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exact::NumericScalar;
    use crate::model::{DependencyScope, MissingDependency, UNSET_VIOLATION};
    use crate::reference_cases;

    #[test]
    fn one_checked_engine_serves_every_analysis_family() {
        let engine = PhonologicalEngine::new();
        let ot = reference_cases::prince_smolensky_ot();
        assert_eq!(
            engine
                .evaluate(&ot.source, EvaluatorKind::Ot, 1.0)
                .expect("OT forms")
                .winner_indices,
            [0]
        );

        let hg = reference_cases::pater_hg();
        assert_eq!(
            engine
                .evaluate(&hg.source, EvaluatorKind::HarmonicGrammar, 1.0)
                .expect("HG forms")
                .winner_indices,
            [1]
        );

        let maxent = reference_cases::finite_maxent_smoke();
        let law = engine
            .evaluate(&maxent.source, EvaluatorKind::MaxEnt, 1.0)
            .expect("MaxEnt forms");
        let mass: f64 = law.rows.iter().filter_map(|row| row.probability).sum();
        assert!((mass - 1.0).abs() < 1.0e-12);

        let serial = reference_cases::serial_syllabification_smoke();
        assert!(
            engine
                .serial(&serial.source, &serial.serial, EvaluatorKind::Ot, 1.0)
                .expect("serial ledger forms")
                .formed
        );

        let second_order = reference_cases::dissertation_second_order();
        assert_eq!(
            engine.compare(&second_order).status,
            ComparisonStatus::Discrepant
        );

        let q = reference_cases::finnish_ranking_space();
        assert!(
            engine
                .q_clone_audit(&q.source, 0, &[], EvaluatorKind::Ot, 1.0)
                .expect("Q audit forms")
                .support_conservative
        );
    }

    #[test]
    fn unset_violation_cells_are_not_evaluated_as_counts() {
        let mut document = reference_cases::prince_smolensky_ot();
        document.source.candidates[0].violations[0] = UNSET_VIOLATION;
        let problem = PhonologicalEngine::new()
            .evaluate(&document.source, EvaluatorKind::Ot, 1.0)
            .expect_err("an unset analyst cell must block evaluation");
        assert_eq!(problem.code, "PE-FORM-VIOLATION-UNSET");
        assert_eq!(problem.coordinate, "candidate[0].violations[0]");
    }

    #[test]
    fn constraint_definition_text_never_changes_the_violation_ledger() {
        let mut document = reference_cases::prince_smolensky_ot();
        let expected = PhonologicalEngine::new()
            .evaluate(&document.source, EvaluatorKind::Ot, 1.0)
            .expect("stored ledger forms");
        document.source.constraints[0].definition = "unknown-operation()".to_owned();
        let observed = PhonologicalEngine::new()
            .evaluate(&document.source, EvaluatorKind::Ot, 1.0)
            .expect("definition is prose, not executable content");
        assert_eq!(observed, expected);
    }

    #[test]
    fn malformed_second_order_input_returns_not_evaluated() {
        let mut document = reference_cases::dissertation_second_order();
        document.target.candidates[0].violations.clear();
        let result = PhonologicalEngine::new().compare(&document);
        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        assert_eq!(
            result.refusal.expect("indexed refusal").code,
            "PE-FORM-MATRIX"
        );
    }

    fn mixed_ot_hg_document() -> ConvalgenDocument {
        let mut document = reference_cases::pater_hg();
        for tableau in [&mut document.source, &mut document.target] {
            tableau.constraints[0].weight = Some(NumericScalar::integer(1));
            tableau.constraints[1].weight = Some(NumericScalar::integer(10));
            tableau.constraints[2].enabled = false;
            tableau.candidates[0].violations = vec![1, 0, 0];
            tableau.candidates[1].violations = vec![0, 1, 0];
        }
        document.source.evaluator = Some(EvaluatorKind::Ot);
        document.target.evaluator = Some(EvaluatorKind::HarmonicGrammar);
        document.second_order.query = crate::model::QueryKind::WinnerSet;
        document.second_order.answer_sort = "winner set".to_owned();
        document.second_order.scope = "both registered candidates".to_owned();
        document.second_order.transformation = "change evaluator from OT to HG".to_owned();
        document.second_order.transport = "identity".to_owned();
        document
    }

    #[test]
    fn second_order_uses_each_tableaus_evaluator_for_a_mixed_ot_hg_discrepancy() {
        let document = mixed_ot_hg_document();
        let result = PhonologicalEngine::new().compare(&document);
        assert_eq!(result.status, ComparisonStatus::Discrepant);
        assert_eq!(result.source_answer, [vec!["devoiced".to_owned()]]);
        assert_eq!(result.target_answer, [vec!["faithful".to_owned()]]);
    }

    #[test]
    fn second_order_can_preserve_a_common_discrete_answer_across_ot_and_hg() {
        let mut document = reference_cases::pater_hg();
        document.source.evaluator = Some(EvaluatorKind::Ot);
        document.target.evaluator = Some(EvaluatorKind::HarmonicGrammar);
        document.second_order.query = crate::model::QueryKind::WinnerSet;
        document.second_order.answer_sort = "winner set".to_owned();
        document.second_order.transformation = "change evaluator from OT to HG".to_owned();
        document.second_order.transport = "identity".to_owned();

        let result = PhonologicalEngine::new().compare(&document);
        assert_eq!(result.status, ComparisonStatus::Preserved);
        let evidence = result
            .certificate
            .expect("preservation certificate")
            .evidence;
        assert!(
            evidence
                .iter()
                .any(|item| item.contains("Optimality Theory"))
        );
        assert!(
            evidence
                .iter()
                .any(|item| item.contains("Harmonic Grammar"))
        );
    }

    #[test]
    fn probability_query_refuses_a_non_maxent_target_evaluator() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.source.evaluator = Some(EvaluatorKind::MaxEnt);
        document.target.evaluator = Some(EvaluatorKind::Ot);
        document.second_order.query = crate::model::QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();

        let result = PhonologicalEngine::new().compare(&document);
        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        let refusal = result.refusal.expect("typed evaluator refusal");
        assert_eq!(refusal.code, "QA-EVALUATOR-QUERY");
        assert_eq!(refusal.coordinate, "target.evaluator");
    }

    #[test]
    fn maxent_probability_laws_use_independent_per_tableau_temperatures() {
        // The smoke ledger is intentionally tied and therefore temperature
        // invariant. Tessier's printed ledger has distinct costs, so it is a
        // genuine regression for independent source and target temperatures.
        let mut document = reference_cases::tessier_hg_maxent();
        document.source.evaluator = Some(EvaluatorKind::MaxEnt);
        document.target.evaluator = Some(EvaluatorKind::MaxEnt);
        document.source.temperature = Some(NumericScalar::integer(1));
        document.target.temperature = Some(NumericScalar::integer(2));
        document.second_order.query = crate::model::QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();

        let result = PhonologicalEngine::new().compare(&document);
        assert_eq!(result.status, ComparisonStatus::Discrepant);
        assert_ne!(result.source_answer, result.target_answer);
        assert_ne!(result.source_normalizer, result.target_normalizer);
    }

    #[test]
    fn strict_ot_does_not_invent_a_weight_dependency() {
        let mut document = reference_cases::prince_smolensky_ot();
        document.source.constraints[0].weight = None;
        let engine = PhonologicalEngine::new();
        assert_eq!(
            engine
                .evaluate(&document.source, EvaluatorKind::Ot, 1.0)
                .expect("OT uses strata and marks, not weights")
                .winner_indices,
            [0]
        );
        let refusal = engine
            .evaluate(&document.source, EvaluatorKind::HarmonicGrammar, 1.0)
            .expect_err("HG requires an available weight");
        assert_eq!(refusal.code, "PE-ADMIT-MISSING-WEIGHT");
        assert_eq!(refusal.coordinate, "constraint[0].weight");
    }

    #[test]
    fn tiny_positive_maxent_temperature_is_used_without_flooring() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.source.constraints[0].weight =
            Some(NumericScalar::parse_exact("0.000000001").expect("exact positive weight"));
        for constraint in document.source.constraints.iter_mut().skip(1) {
            constraint.enabled = false;
        }

        let result = PhonologicalEngine::new()
            .evaluate(&document.source, EvaluatorKind::MaxEnt, 1.0e-9)
            .expect("a tiny positive temperature is admissible");
        let expected_weak_probability = 1.0 / (1.0 + (-1.0_f64).exp());

        assert!(
            (result.rows[0].probability.expect("MaxEnt probability") - expected_weak_probability)
                .abs()
                < 1.0e-12
        );
    }

    #[test]
    fn maxent_learning_rejects_an_incompatible_evaluator_override() {
        let mut document = reference_cases::finite_maxent_smoke();
        let tableau = &mut document.dataset[0];
        tableau.evaluator = Some(EvaluatorKind::Ot);
        tableau.constraints[0].weight = None;

        let problem = PhonologicalEngine::new()
            .learn_maxent(std::slice::from_ref(tableau), 1.0, 10)
            .expect_err("an OT-overridden tableau is not admitted to MaxEnt learning");

        assert_eq!(problem.code, "PE-ADMIT-LEARNING-EVALUATOR");
        assert_eq!(problem.stage, EngineStage::Admission);
        assert_eq!(problem.coordinate, "tableau[0].evaluator");
    }

    #[test]
    fn maxent_learning_reports_a_missing_weight_before_dispatch() {
        let mut document = reference_cases::finite_maxent_smoke();
        let tableau = &mut document.dataset[0];
        tableau.evaluator = None;
        tableau.constraints[0].weight = None;

        let problem = PhonologicalEngine::new()
            .learn_maxent(std::slice::from_ref(tableau), 1.0, 10)
            .expect_err("a missing enabled weight is an admission refusal");

        assert_eq!(problem.code, "PE-ADMIT-MISSING-WEIGHT");
        assert_eq!(problem.stage, EngineStage::Admission);
        assert_eq!(problem.coordinate, "tableau[0].constraint[0].weight");
    }

    #[test]
    fn learning_dependency_blocks_learning_but_not_first_order_evaluation() {
        let mut document = reference_cases::prince_smolensky_ot();
        document
            .source
            .missing_dependencies
            .push(MissingDependency {
                code: "PE-ADMIT-MISSING-TRAINING-EVIDENCE".to_owned(),
                stage: DependencyStage::Admission,
                coordinate: "learning.training-evidence".to_owned(),
                scope: DependencyScope::Learning,
                message: "training evidence is unavailable".to_owned(),
                remedy: "supply the registered training evidence".to_owned(),
            });
        let engine = PhonologicalEngine::new();
        engine
            .evaluate(&document.source, EvaluatorKind::Ot, 1.0)
            .expect("a learning-only dependency does not block evaluation");
        let refusal = engine
            .infer_ranking(std::slice::from_ref(&document.source), &[])
            .expect_err("the same dependency blocks the named operation scope");
        assert_eq!(refusal.code, "PE-ADMIT-MISSING-TRAINING-EVIDENCE");
        assert_eq!(refusal.coordinate, "tableau[0].learning.training-evidence");
    }
}
