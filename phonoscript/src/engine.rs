use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use num_bigint::{BigInt, BigUint};
use num_rational::{BigRational, Ratio};
use num_traits::{One, ToPrimitive, Zero};

use crate::exact::NumericScalar;
use crate::model::{
    Candidate, ComparisonMode, ConsumerMode, DependencyStage, EvaluatorKind, NormalizerPolicy,
    QueryKind, ResponseDomain, SecondOrderSettings, SerialMove, SerialSettings, Tableau, TiePolicy,
    UNSET_VIOLATION,
};

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateEvaluation {
    pub candidate: usize,
    /// Exact weighted violation cost when every enabled weight is exact.
    ///
    /// Strict OT has no scalar harmony, and a weighted tableau containing an
    /// explicitly approximate weight has no exact cost certificate.
    pub exact_harmony: Option<BigRational>,
    pub harmony: f64,
    pub probability: Option<f64>,
    pub winner: bool,
    pub fatal_constraint: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableauEvaluation {
    pub rows: Vec<CandidateEvaluation>,
    /// The evaluator's complete set of co-optimal candidates before the
    /// document's explicit tie policy is applied.
    pub native_winner_indices: Vec<usize>,
    pub winner_indices: Vec<usize>,
    pub ordered_strata: Vec<Vec<usize>>,
    /// True only when the native optimum is non-singleton and the tableau
    /// explicitly requires a unique winner.
    pub tie_unresolved: bool,
}

/// Return the violation count entered by the phonologist.
///
/// Constraint names, definitions, candidate forms, and structured records are
/// never interpreted as violation-counting programs.
pub fn resolved_violation(
    tableau: &Tableau,
    candidate: &Candidate,
    constraint_index: usize,
) -> Result<u16, String> {
    tableau
        .constraints
        .get(constraint_index)
        .ok_or_else(|| "constraint index is outside the tableau".to_owned())?;
    let mark = candidate
        .violations
        .get(constraint_index)
        .copied()
        .ok_or_else(|| "candidate row is shorter than the constraint register".to_owned())?;
    if mark == UNSET_VIOLATION {
        return Err("violation count is unset; the phonologist must enter it".to_owned());
    }
    Ok(mark)
}

fn ot_key(tableau: &Tableau, candidate: &Candidate) -> Vec<u64> {
    let max_stratum = tableau
        .constraints
        .iter()
        .map(|constraint| constraint.stratum)
        .max()
        .unwrap_or(0);
    (0..=max_stratum)
        .map(|stratum| {
            tableau
                .constraints
                .iter()
                .enumerate()
                .filter(|(_, constraint)| constraint.enabled && constraint.stratum == stratum)
                .map(|(index, _)| {
                    u64::from(resolved_violation(tableau, candidate, index).unwrap_or(u16::MAX))
                })
                .sum()
        })
        .collect()
}

fn harmony(tableau: &Tableau, candidate: &Candidate, kind: EvaluatorKind) -> f64 {
    tableau
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, constraint)| constraint.enabled)
        .map(|(index, constraint)| {
            let weight = match &constraint.weight {
                Some(weight) => weight
                    .to_f64_center()
                    .expect("checked evaluation requires a representable weight"),
                // Strict OT is defined only by strata and violation marks;
                // a missing weight is not an evaluator dependency.
                None if kind == EvaluatorKind::Ot => 0.0,
                None => panic!("checked weighted evaluation requires every enabled weight"),
            };
            weight * f64::from(resolved_violation(tableau, candidate, index).unwrap_or(u16::MAX))
        })
        .sum()
}

pub(crate) fn evaluate(
    tableau: &Tableau,
    kind: EvaluatorKind,
    temperature: f64,
) -> TableauEvaluation {
    if tableau.candidates.is_empty() {
        return TableauEvaluation {
            rows: Vec::new(),
            native_winner_indices: Vec::new(),
            winner_indices: Vec::new(),
            ordered_strata: Vec::new(),
            tie_unresolved: false,
        };
    }

    // Compile the enabled weight register once per evaluation. Converting the
    // same exact scalar for every candidate was the dominant cost for ordinary
    // medium-sized MaxEnt tableaux.
    let enabled_weights: Vec<(usize, f64)> = tableau
        .constraints
        .iter()
        .enumerate()
        .filter(|(_, constraint)| constraint.enabled)
        .map(|(index, constraint)| {
            let weight = match &constraint.weight {
                Some(weight) => weight
                    .to_f64_center()
                    .expect("checked evaluation requires a representable weight"),
                None if kind == EvaluatorKind::Ot => 0.0,
                None => panic!("checked weighted evaluation requires every enabled weight"),
            };
            (index, weight)
        })
        .collect();
    let harmonies: Vec<f64> = tableau
        .candidates
        .iter()
        .map(|candidate| {
            enabled_weights
                .iter()
                .map(|(index, weight)| {
                    *weight
                        * f64::from(
                            resolved_violation(tableau, candidate, *index).unwrap_or(u16::MAX),
                        )
                })
                .sum()
        })
        .collect();
    let exact_weights: Option<Vec<(usize, &BigRational)>> = (kind != EvaluatorKind::Ot)
        .then(|| {
            tableau
                .constraints
                .iter()
                .enumerate()
                .filter(|(_, constraint)| constraint.enabled)
                .map(|(index, constraint)| {
                    constraint
                        .weight
                        .as_ref()
                        .and_then(|weight| weight.exact_value().ok())
                        .map(|weight| (index, weight))
                })
                .collect::<Option<Vec<_>>>()
        })
        .flatten();
    let exact_harmonies: Vec<Option<BigRational>> = match exact_weights {
        None => vec![None; tableau.candidates.len()],
        Some(weights) if weights.iter().all(|(_, weight)| weight.denom().is_one()) => tableau
            .candidates
            .iter()
            .map(|candidate| {
                let mut total = BigInt::zero();
                for (index, weight) in &weights {
                    let violation =
                        resolved_violation(tableau, candidate, *index).unwrap_or(u16::MAX);
                    match violation {
                        0 => {}
                        1 => total += weight.numer(),
                        _ => total += weight.numer() * BigInt::from(violation),
                    }
                }
                Some(BigRational::from_integer(total))
            })
            .collect(),
        Some(weights) => tableau
            .candidates
            .iter()
            .map(|candidate| {
                let mut total = BigRational::zero();
                for (index, weight) in &weights {
                    let violation =
                        resolved_violation(tableau, candidate, *index).unwrap_or(u16::MAX);
                    match violation {
                        0 => {}
                        1 => total += *weight,
                        _ => {
                            total += (*weight).clone()
                                * BigRational::from_integer(BigInt::from(violation));
                        }
                    }
                }
                Some(total)
            })
            .collect(),
    };
    let scale = temperature;
    let maxent_costs: Vec<f64> = if kind == EvaluatorKind::MaxEnt {
        harmonies
            .iter()
            .zip(&tableau.candidates)
            .map(|(score, candidate)| {
                score / scale
                    - candidate
                        .base_mass
                        .to_f64_center()
                        .expect("checked candidate mass")
                        .ln()
            })
            .collect()
    } else {
        Vec::new()
    };
    let ot_keys: Vec<Vec<u64>> = if kind == EvaluatorKind::Ot {
        tableau
            .candidates
            .iter()
            .map(|candidate| ot_key(tableau, candidate))
            .collect()
    } else {
        Vec::new()
    };
    let mut order: Vec<usize> = (0..tableau.candidates.len()).collect();
    match kind {
        EvaluatorKind::Ot => order.sort_by(|left, right| {
            ot_keys[*left].cmp(&ot_keys[*right]).then_with(|| {
                tableau.candidates[*left]
                    .name
                    .cmp(&tableau.candidates[*right].name)
            })
        }),
        EvaluatorKind::HarmonicGrammar => order.sort_by(|left, right| {
            match (&exact_harmonies[*left], &exact_harmonies[*right]) {
                (Some(left_cost), Some(right_cost)) => left_cost.cmp(right_cost),
                _ => harmonies[*left].total_cmp(&harmonies[*right]),
            }
            .then_with(|| {
                tableau.candidates[*left]
                    .name
                    .cmp(&tableau.candidates[*right].name)
            })
        }),
        EvaluatorKind::MaxEnt => order.sort_by(|left, right| {
            maxent_costs[*left]
                .total_cmp(&maxent_costs[*right])
                .then_with(|| {
                    tableau.candidates[*left]
                        .name
                        .cmp(&tableau.candidates[*right].name)
                })
        }),
    }
    let tied = |left: usize, right: usize| match kind {
        EvaluatorKind::Ot => ot_keys[left] == ot_keys[right],
        EvaluatorKind::HarmonicGrammar => match (&exact_harmonies[left], &exact_harmonies[right]) {
            (Some(left_cost), Some(right_cost)) => left_cost == right_cost,
            _ => harmonies[left].total_cmp(&harmonies[right]).is_eq(),
        },
        EvaluatorKind::MaxEnt => maxent_costs[left].total_cmp(&maxent_costs[right]).is_eq(),
    };
    let mut ordered_strata: Vec<Vec<usize>> = Vec::new();
    for candidate in order {
        if let Some(last) = ordered_strata.last_mut()
            && tied(last[0], candidate)
        {
            last.push(candidate);
        } else {
            ordered_strata.push(vec![candidate]);
        }
    }
    let native_winner_indices = ordered_strata.first().cloned().unwrap_or_default();
    let tie_unresolved =
        native_winner_indices.len() > 1 && tableau.tie_policy_kind() == TiePolicy::RequireUnique;
    let winner_indices = match tableau.tie_policy_kind() {
        TiePolicy::RetainAll => native_winner_indices.clone(),
        TiePolicy::FirstListed => native_winner_indices
            .iter()
            .copied()
            .min()
            .into_iter()
            .collect(),
        TiePolicy::RequireUnique if tie_unresolved => Vec::new(),
        TiePolicy::RequireUnique => native_winner_indices.clone(),
    };

    let probabilities = if kind == EvaluatorKind::MaxEnt {
        // `maxent_costs` already contains `h/T - ln(base mass)`. Reusing it
        // avoids independently rounding a duplicate score and logarithm while
        // preserving the same log-sum-exp normalization.
        let least = maxent_costs.iter().copied().fold(f64::INFINITY, f64::min);
        let masses: Vec<f64> = maxent_costs
            .iter()
            .map(|cost| (least - cost).exp())
            .collect();
        let normalizer: f64 = masses.iter().sum();
        Some(
            masses
                .iter()
                .map(|mass| mass / normalizer)
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    let rows = tableau
        .candidates
        .iter()
        .enumerate()
        .map(|(index, _candidate)| {
            let fatal_constraint = if kind == EvaluatorKind::Ot
                && !winner_indices.is_empty()
                && !winner_indices.contains(&index)
            {
                let winner = winner_indices[0];
                let fatal_stratum = ot_keys[index]
                    .iter()
                    .zip(&ot_keys[winner])
                    .position(|(candidate, optimum)| candidate > optimum);
                fatal_stratum.and_then(|stratum| {
                    tableau
                        .constraints
                        .iter()
                        .enumerate()
                        .find_map(|(constraint, item)| {
                            (item.stratum == stratum
                                && resolved_violation(
                                    tableau,
                                    &tableau.candidates[index],
                                    constraint,
                                )
                                .unwrap_or(u16::MAX)
                                    > resolved_violation(
                                        tableau,
                                        &tableau.candidates[winner],
                                        constraint,
                                    )
                                    .unwrap_or(u16::MAX))
                            .then_some(constraint)
                        })
                })
            } else {
                None
            };
            CandidateEvaluation {
                candidate: index,
                exact_harmony: exact_harmonies[index].clone(),
                harmony: harmonies[index],
                probability: probabilities.as_ref().map(|values| values[index]),
                winner: winner_indices.contains(&index),
                fatal_constraint,
            }
        })
        .collect();
    TableauEvaluation {
        rows,
        native_winner_indices,
        winner_indices,
        ordered_strata,
        tie_unresolved,
    }
}

pub(crate) fn query_answer(
    tableau: &Tableau,
    result: &TableauEvaluation,
    query: QueryKind,
) -> Vec<Vec<String>> {
    match query {
        QueryKind::WinnerSet => vec![
            result
                .winner_indices
                .iter()
                .map(|index| tableau.candidates[*index].name.clone())
                .collect(),
        ],
        QueryKind::SurfaceWinnerSet => {
            let mut outputs = result
                .winner_indices
                .iter()
                .map(|index| tableau.candidates[*index].form.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            outputs.sort();
            vec![outputs]
        }
        QueryKind::CompleteOrder => result
            .ordered_strata
            .iter()
            .map(|stratum| {
                stratum
                    .iter()
                    .map(|index| tableau.candidates[*index].name.clone())
                    .collect()
            })
            .collect(),
        QueryKind::ProbabilityLaw => {
            let mut law: Vec<String> = result
                .rows
                .iter()
                .filter_map(|row| {
                    row.probability.map(|probability| {
                        format!(
                            "{}={probability:.12}",
                            tableau.candidates[row.candidate].name
                        )
                    })
                })
                .collect();
            law.sort();
            vec![law]
        }
        QueryKind::CandidateSupport => {
            let mut support: Vec<String> = tableau
                .candidates
                .iter()
                .map(|candidate| candidate.name.clone())
                .collect();
            support.sort();
            vec![support]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonStatus {
    Preserved,
    Discrepant,
    NotEvaluated,
}

impl ComparisonStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Preserved => "PRESERVED",
            Self::Discrepant => "DISCREPANCY",
            Self::NotEvaluated => "NOT EVALUATED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStage {
    Formation,
    Admission,
    Evaluation,
    Certification,
}

impl ContractStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Formation => "formation",
            Self::Admission => "admission",
            Self::Evaluation => "evaluation",
            Self::Certification => "certification",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonRefusal {
    pub code: String,
    pub stage: ContractStage,
    pub coordinate: String,
    pub message: String,
    pub remedy: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscrepancyRecord {
    pub coordinate: String,
    pub source: String,
    pub target: String,
    pub difference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservationCertificate {
    pub statement: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecondOrderResult {
    pub status: ComparisonStatus,
    pub source_answer: Vec<Vec<String>>,
    pub transported_source_answer: Vec<Vec<String>>,
    pub target_answer: Vec<Vec<String>>,
    pub discrepancies: Vec<DiscrepancyRecord>,
    pub refusal: Option<ComparisonRefusal>,
    pub certificate: Option<PreservationCertificate>,
    pub source_normalizer: Option<String>,
    pub target_normalizer: Option<String>,
}

impl SecondOrderResult {
    pub const fn conservative(&self) -> bool {
        matches!(self.status, ComparisonStatus::Preserved)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ComputedAnswer {
    Discrete(Vec<Vec<String>>),
    Probability(BTreeMap<String, f64>),
}

impl ComputedAnswer {
    fn rendered(&self) -> Vec<Vec<String>> {
        match self {
            Self::Discrete(value) => value.clone(),
            Self::Probability(value) => vec![
                value
                    .iter()
                    .map(|(candidate, probability)| format!("{candidate}={probability:.12}"))
                    .collect(),
            ],
        }
    }
}

#[derive(Debug, Clone)]
struct TransportPlan {
    assignments: HashMap<String, String>,
    fusion: bool,
}

impl TransportPlan {
    fn map(&self, value: &str) -> String {
        self.assignments
            .get(value)
            .cloned()
            .unwrap_or_else(|| value.to_owned())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ExactPolynomial {
    terms: BTreeMap<BigRational, BigRational>,
}

impl ExactPolynomial {
    fn monomial(exponent: BigRational, coefficient: BigRational) -> Self {
        let mut terms = BTreeMap::new();
        if !coefficient.is_zero() {
            terms.insert(exponent, coefficient);
        }
        Self { terms }
    }

    fn add_assign(&mut self, other: &Self) {
        for (exponent, coefficient) in &other.terms {
            let entry = self.terms.entry(exponent.clone()).or_default();
            *entry += coefficient;
            if entry.is_zero() {
                self.terms.remove(exponent);
            }
        }
    }

    fn multiply(&self, other: &Self) -> Self {
        let mut result = Self::default();
        for (left_exponent, left_coefficient) in &self.terms {
            for (right_exponent, right_coefficient) in &other.terms {
                let exponent = left_exponent + right_exponent;
                let coefficient = left_coefficient * right_coefficient;
                let entry = result.terms.entry(exponent).or_default();
                *entry += coefficient;
            }
        }
        result.terms.retain(|_, coefficient| !coefficient.is_zero());
        result
    }

    fn summary(&self) -> String {
        if self.terms.is_empty() {
            return "0".to_owned();
        }
        if self.terms.len() == 1 {
            let (exponent, coefficient) = self.terms.first_key_value().expect("one term");
            return format!("{coefficient}·exp({exponent})");
        }
        format!("{} exact exponential terms", self.terms.len())
    }
}

#[derive(Debug, Clone)]
struct ExactProbabilityLaw {
    masses: BTreeMap<String, ExactPolynomial>,
    normalizer: ExactPolynomial,
}

fn refusal(
    code: &str,
    stage: ContractStage,
    coordinate: &str,
    message: impl Into<String>,
    remedy: impl Into<String>,
) -> ComparisonRefusal {
    ComparisonRefusal {
        code: code.to_owned(),
        stage,
        coordinate: coordinate.to_owned(),
        message: message.into(),
        remedy: remedy.into(),
    }
}

fn not_evaluated(
    refusal: ComparisonRefusal,
    source: Option<&ComputedAnswer>,
    transported: Option<&ComputedAnswer>,
    target: Option<&ComputedAnswer>,
    source_normalizer: Option<String>,
    target_normalizer: Option<String>,
) -> SecondOrderResult {
    SecondOrderResult {
        status: ComparisonStatus::NotEvaluated,
        source_answer: source.map(ComputedAnswer::rendered).unwrap_or_default(),
        transported_source_answer: transported
            .map(ComputedAnswer::rendered)
            .unwrap_or_default(),
        target_answer: target.map(ComputedAnswer::rendered).unwrap_or_default(),
        discrepancies: Vec::new(),
        refusal: Some(refusal),
        certificate: None,
        source_normalizer,
        target_normalizer,
    }
}

fn parse_transport(text: &str) -> Result<TransportPlan, ComparisonRefusal> {
    let declaration = text.trim();
    let lower = declaration.to_ascii_lowercase();
    if lower.starts_with("identity") {
        return Ok(TransportPlan {
            assignments: HashMap::new(),
            fusion: false,
        });
    }
    let (prefix, fusion) = if lower.starts_with("rename ") {
        ("rename ", false)
    } else if lower.starts_with("renaming ") {
        ("renaming ", false)
    } else if lower.starts_with("fusion ") {
        ("fusion ", true)
    } else {
        return Err(refusal(
            "QF-TRANSPORT-SYNTAX",
            ContractStage::Formation,
            "transport",
            "the transport is neither identity, rename, nor fusion",
            "Declare `identity`, `rename source=target`, or `fusion a+b=ab; mass-preserving`.",
        ));
    };
    if fusion && !lower.contains("mass-preserving") {
        return Err(refusal(
            "QA-FUSION-MASS",
            ContractStage::Admission,
            "transport.mass-preservation",
            "a fusion was declared without a mass-preservation obligation",
            "Append `; mass-preserving` and ensure every source mass is assigned exactly once.",
        ));
    }
    let qualifier = lower.find("mass-preserving").unwrap_or(declaration.len());
    let body = declaration[prefix.len()..qualifier]
        .trim()
        .trim_end_matches(';')
        .trim();
    if body.is_empty() {
        return Err(refusal(
            "QF-TRANSPORT-EMPTY",
            ContractStage::Formation,
            "transport.mapping",
            "the declared transport has no mapping clauses",
            "Provide at least one `source=target` mapping.",
        ));
    }
    let mut assignments = HashMap::new();
    for clause in body.split(',') {
        let (left, right) = clause.split_once('=').ok_or_else(|| {
            refusal(
                "QF-TRANSPORT-CLAUSE",
                ContractStage::Formation,
                "transport.mapping",
                format!("mapping clause `{}` has no `=`", clause.trim()),
                "Use `source=target`; fusion sources are joined with `+`.",
            )
        })?;
        let target = right.trim();
        if target.is_empty() {
            return Err(refusal(
                "QF-TRANSPORT-TARGET",
                ContractStage::Formation,
                "transport.codomain",
                "a mapping clause has an empty target",
                "Name the target response identity after `=`.",
            ));
        }
        let sources: Vec<&str> = if fusion {
            left.split('+').map(str::trim).collect()
        } else {
            vec![left.trim()]
        };
        for source in sources {
            if source.is_empty()
                || assignments
                    .insert(source.to_owned(), target.to_owned())
                    .is_some()
            {
                return Err(refusal(
                    "QA-TRANSPORT-FUNCTION",
                    ContractStage::Admission,
                    "transport.domain",
                    "a source response is empty or mapped more than once",
                    "Make the transport a total single-valued function on the source response.",
                ));
            }
        }
    }
    if !fusion {
        let mut targets = HashSet::new();
        if assignments.values().any(|target| !targets.insert(target)) {
            return Err(refusal(
                "QA-RENAME-INJECTIVE",
                ContractStage::Admission,
                "transport.injectivity",
                "a candidate renaming maps two declared source identities to one target identity",
                "Use an injective rename or declare a mass-preserving fusion for probability laws.",
            ));
        }
    }
    Ok(TransportPlan {
        assignments,
        fusion,
    })
}

fn terminal_answer(
    tableau: &Tableau,
    kind: EvaluatorKind,
    query: QueryKind,
    temperature: f64,
) -> ComputedAnswer {
    let evaluated = evaluate(tableau, kind, temperature);
    if query == QueryKind::ProbabilityLaw {
        let probabilities = evaluated
            .rows
            .iter()
            .filter_map(|row| {
                row.probability.map(|probability| {
                    (tableau.candidates[row.candidate].name.clone(), probability)
                })
            })
            .collect();
        ComputedAnswer::Probability(probabilities)
    } else {
        let mut answer = query_answer(tableau, &evaluated, query);
        for tier in &mut answer {
            tier.sort();
        }
        ComputedAnswer::Discrete(answer)
    }
}

fn trajectory_answer(
    constraints: &[crate::model::Constraint],
    settings: &SerialSettings,
    kind: EvaluatorKind,
    temperature: f64,
    coordinate: &str,
) -> Result<ComputedAnswer, ComparisonRefusal> {
    let result = evaluate_serial(
        constraints,
        &settings.moves,
        &settings.start,
        kind,
        temperature,
        settings.maximum_steps,
    );
    if !result.formed {
        return Err(refusal(
            "QE-TRAJECTORY",
            ContractStage::Evaluation,
            coordinate,
            format!(
                "the independently calculated trajectory was refused: {}",
                result.stopped
            ),
            "Supply a nonempty initial form, identity candidates, and a terminating serial ledger.",
        ));
    }
    Ok(ComputedAnswer::Discrete(vec![result.path]))
}

fn apply_transport(
    answer: &ComputedAnswer,
    plan: &TransportPlan,
) -> Result<ComputedAnswer, ComparisonRefusal> {
    match answer {
        ComputedAnswer::Discrete(tiers) => {
            if plan.fusion {
                return Err(refusal(
                    "QA-FUSION-TYPE",
                    ContractStage::Admission,
                    "transport.answer-type",
                    "probability-mass fusion cannot transport a discrete response",
                    "Use an injective rename for discrete answers or query a MaxEnt probability law.",
                ));
            }
            let mapped: Vec<Vec<String>> = tiers
                .iter()
                .map(|tier| tier.iter().map(|value| plan.map(value)).collect())
                .collect();
            let source_support: BTreeSet<&str> =
                tiers.iter().flatten().map(String::as_str).collect();
            let mapped_support: BTreeSet<String> = mapped.iter().flatten().cloned().collect();
            if source_support.len() != mapped_support.len() {
                return Err(refusal(
                    "QA-RENAME-COLLISION",
                    ContractStage::Admission,
                    "transport.injectivity",
                    "the rename collides with an unchanged source identity",
                    "Declare an injective mapping over the complete source response support.",
                ));
            }
            Ok(ComputedAnswer::Discrete(mapped))
        }
        ComputedAnswer::Probability(law) => {
            let mut transported = BTreeMap::<String, f64>::new();
            for (candidate, probability) in law {
                *transported.entry(plan.map(candidate)).or_default() += probability;
            }
            Ok(ComputedAnswer::Probability(transported))
        }
    }
}

fn apply_consumer(
    answer: &ComputedAnswer,
    consumer: &str,
    domain: ResponseDomain,
    exact_probability: bool,
) -> Result<ComputedAnswer, ComparisonRefusal> {
    match consumer.trim().to_ascii_lowercase().as_str() {
        "identity" => Ok(answer.clone()),
        "support-cardinality" => {
            let count = match answer {
                ComputedAnswer::Discrete(tiers) => {
                    tiers.iter().flatten().collect::<BTreeSet<_>>().len()
                }
                ComputedAnswer::Probability(law) => law.len(),
            };
            Ok(ComputedAnswer::Discrete(vec![vec![count.to_string()]]))
        }
        "winner-set" => match answer {
            ComputedAnswer::Discrete(tiers) => Ok(ComputedAnswer::Discrete(
                tiers.first().cloned().into_iter().collect(),
            )),
            ComputedAnswer::Probability(law) => {
                if exact_probability {
                    return Err(refusal(
                        "QC-EXACT-ARGMAX",
                        ContractStage::Certification,
                        "consumer.winner-set",
                        "an exact argmax certificate is not available from rounded display probabilities",
                        "Use direct exact probability comparison or choose approximate mode explicitly.",
                    ));
                }
                let maximum = law.values().copied().fold(f64::NEG_INFINITY, f64::max);
                let winners = law
                    .iter()
                    .filter(|(_, value)| (**value - maximum).abs() <= 1.0e-12)
                    .map(|(candidate, _)| candidate.clone())
                    .collect();
                Ok(ComputedAnswer::Discrete(vec![winners]))
            }
        },
        "terminal-output" if domain == ResponseDomain::Trajectory => match answer {
            ComputedAnswer::Discrete(tiers) => Ok(ComputedAnswer::Discrete(vec![vec![
                tiers
                    .first()
                    .and_then(|path| path.last())
                    .cloned()
                    .unwrap_or_default(),
            ]])),
            ComputedAnswer::Probability(_) => unreachable!("trajectory is discrete"),
        },
        _ => Err(refusal(
            "QF-CONSUMER",
            ContractStage::Formation,
            "consumer",
            format!("consumer `{}` is missing or undefined", consumer.trim()),
            "Declare `identity`, `winner-set`, `support-cardinality`, or trajectory `terminal-output`.",
        )),
    }
}

fn exact_probability_law(
    tableau: &Tableau,
    temperature: &NumericScalar,
) -> Result<ExactProbabilityLaw, ComparisonRefusal> {
    let temperature = temperature.exact_value().map_err(|_| {
        refusal(
            "QC-APPROXIMATE-TEMPERATURE",
            ContractStage::Certification,
            "exactness.temperature",
            "exact probability certification requires a genuinely exact stored temperature",
            "Store the temperature as an exact integer, rational, or finite decimal, or select approximate comparison.",
        )
    })?;
    if temperature.is_zero() {
        return Err(refusal(
            "QC-TEMPERATURE-ZERO",
            ContractStage::Certification,
            "exactness.temperature",
            "zero temperature cannot normalize a MaxEnt law",
            "Enter a positive temperature.",
        ));
    }
    let mut masses = BTreeMap::new();
    let mut normalizer = ExactPolynomial::default();
    for candidate in &tableau.candidates {
        let base = candidate.base_mass.exact_value().map_err(|_| {
            refusal(
                "QC-APPROXIMATE-BASE-MASS",
                ContractStage::Certification,
                &format!("candidate[{}].base-mass", candidate.name),
                format!(
                    "base mass for `{}` is explicitly approximate",
                    candidate.name
                ),
                "Store a genuinely exact base mass or select approximate comparison.",
            )
        })?;
        let mut score = BigRational::zero();
        for (index, constraint) in tableau.constraints.iter().enumerate() {
            if !constraint.enabled {
                continue;
            }
            let weight = constraint.weight.as_ref().ok_or_else(|| {
                refusal(
                    "QE-MISSING-WEIGHT",
                    ContractStage::Admission,
                    &format!("constraint[{index}].weight"),
                    format!("weight for `{}` is unavailable", constraint.name),
                    "Supply the fitted weight or retain the tableau as a nonevaluable mark ledger.",
                )
            })?;
            let weight = weight.exact_value().map_err(|_| {
                refusal(
                    "QC-APPROXIMATE-WEIGHT",
                    ContractStage::Certification,
                    &format!("constraint[{index}].weight"),
                    format!("weight for `{}` is explicitly approximate", constraint.name),
                    "Store a genuinely exact weight or select approximate comparison.",
                )
            })?;
            let violation = resolved_violation(tableau, candidate, index).map_err(|message| {
                refusal(
                    "QE-VIOLATION",
                    ContractStage::Evaluation,
                    "candidate.violation",
                    message,
                    "Repair the analyst-supplied violation ledger.",
                )
            })?;
            score += weight * BigRational::from_integer(BigInt::from(violation));
        }
        let mass = ExactPolynomial::monomial(-score / temperature, base.clone());
        normalizer.add_assign(&mass);
        masses.insert(candidate.name.clone(), mass);
    }
    Ok(ExactProbabilityLaw { masses, normalizer })
}

fn transport_exact_law(law: &ExactProbabilityLaw, plan: &TransportPlan) -> ExactProbabilityLaw {
    let mut masses = BTreeMap::<String, ExactPolynomial>::new();
    for (candidate, mass) in &law.masses {
        masses
            .entry(plan.map(candidate))
            .or_default()
            .add_assign(mass);
    }
    ExactProbabilityLaw {
        masses,
        normalizer: law.normalizer.clone(),
    }
}

fn maxent_log_normalizer(tableau: &Tableau, temperature: f64) -> f64 {
    let scale = temperature;
    let logs: Vec<f64> = tableau
        .candidates
        .iter()
        .map(|candidate| {
            candidate
                .base_mass
                .to_f64_center()
                .expect("checked candidate mass")
                .ln()
                - harmony(tableau, candidate, EvaluatorKind::MaxEnt) / scale
        })
        .collect();
    let largest = logs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    largest
        + logs
            .iter()
            .map(|value| (value - largest).exp())
            .sum::<f64>()
            .ln()
}

fn discrete_discrepancies(
    source: &[Vec<String>],
    target: &[Vec<String>],
) -> Vec<DiscrepancyRecord> {
    let mut discrepancies = Vec::new();
    let length = source.len().max(target.len());
    for index in 0..length {
        let left = source.get(index).cloned().unwrap_or_default();
        let right = target.get(index).cloned().unwrap_or_default();
        if left != right {
            discrepancies.push(DiscrepancyRecord {
                coordinate: format!("response.tier[{}]", index + 1),
                source: format!("{{{}}}", left.join(", ")),
                target: format!("{{{}}}", right.join(", ")),
                difference: "ordered response tier differs".to_owned(),
            });
        }
    }
    discrepancies
}

fn approximate_probability_discrepancies(
    source: &BTreeMap<String, f64>,
    target: &BTreeMap<String, f64>,
    mode: ComparisonMode,
    tolerance: f64,
    grid_step: f64,
) -> Vec<DiscrepancyRecord> {
    let support: BTreeSet<&String> = source.keys().chain(target.keys()).collect();
    let mut discrepancies = Vec::new();
    for candidate in support {
        let left = source.get(candidate).copied().unwrap_or(0.0);
        let right = target.get(candidate).copied().unwrap_or(0.0);
        let (different, difference) = match mode {
            ComparisonMode::Approximate => (
                (left - right).abs() > tolerance,
                format!(
                    "absolute difference {:.12} exceeds tolerance {tolerance:.12}",
                    (left - right).abs()
                ),
            ),
            ComparisonMode::Grid => {
                let left_cell = (left / grid_step).round() as i64;
                let right_cell = (right / grid_step).round() as i64;
                (
                    left_cell != right_cell,
                    format!(
                        "grid cells {left_cell} and {right_cell} differ at step {grid_step:.12}"
                    ),
                )
            }
            ComparisonMode::Exact => unreachable!("exact comparison uses symbolic laws"),
        };
        if different {
            discrepancies.push(DiscrepancyRecord {
                coordinate: format!("probability[{candidate}]"),
                source: format!("{left:.12}"),
                target: format!("{right:.12}"),
                difference,
            });
        }
    }
    discrepancies
}

fn exact_probability_discrepancies(
    source: &ExactProbabilityLaw,
    target: &ExactProbabilityLaw,
) -> Vec<DiscrepancyRecord> {
    let support: BTreeSet<&String> = source.masses.keys().chain(target.masses.keys()).collect();
    let zero = ExactPolynomial::default();
    let mut discrepancies = Vec::new();
    for candidate in support {
        let left = source.masses.get(candidate).unwrap_or(&zero);
        let right = target.masses.get(candidate).unwrap_or(&zero);
        if left.multiply(&target.normalizer) != right.multiply(&source.normalizer) {
            discrepancies.push(DiscrepancyRecord {
                coordinate: format!("probability[{candidate}]"),
                source: left.summary(),
                target: right.summary(),
                difference: "normalized exact exponential-polynomial masses differ".to_owned(),
            });
        }
    }
    discrepancies
}

/// Execute the dissertation's typed comparison contract. Source and target
/// responses are always calculated independently; the transport is applied
/// only after both calculations. Every incomplete contract returns a typed
/// `NotEvaluated` result rather than false, NaN, or a generic runtime error.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EvaluationContext<'a> {
    pub kind: EvaluatorKind,
    pub temperature: &'a NumericScalar,
}

#[cfg(test)]
pub(crate) fn compare_tableaux_with_contract(
    source: &Tableau,
    target: &Tableau,
    source_serial: &SerialSettings,
    target_serial: &SerialSettings,
    kind: EvaluatorKind,
    temperature: &NumericScalar,
    settings: &SecondOrderSettings,
) -> SecondOrderResult {
    let context = EvaluationContext { kind, temperature };
    compare_tableaux_with_contexts(
        source,
        target,
        source_serial,
        target_serial,
        context,
        context,
        settings,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compare_tableaux_with_contexts(
    source: &Tableau,
    target: &Tableau,
    source_serial: &SerialSettings,
    target_serial: &SerialSettings,
    source_context: EvaluationContext<'_>,
    target_context: EvaluationContext<'_>,
    settings: &SecondOrderSettings,
) -> SecondOrderResult {
    if settings.comparison_mode == ComparisonMode::Exact {
        for (side, tableau) in [("source", source), ("target", target)] {
            if let Some(dependency) = tableau
                .missing_dependencies
                .iter()
                .find(|dependency| dependency.blocks_exact_certification())
            {
                return not_evaluated(
                    refusal(
                        &dependency.code,
                        match dependency.stage {
                            DependencyStage::Formation => ContractStage::Formation,
                            DependencyStage::Admission => ContractStage::Admission,
                        },
                        &format!("{side}.{}", dependency.coordinate),
                        dependency.message.clone(),
                        dependency.remedy.clone(),
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
        }
    }
    let numerical_temperature = |side: &str, temperature: &NumericScalar| {
        temperature
            .to_f64_center()
            .ok()
            .filter(|value| *value > 0.0)
            .ok_or_else(|| {
                refusal(
                    "QA-TEMPERATURE",
                    ContractStage::Admission,
                    &format!("{side}.temperature"),
                    format!(
                        "the stored {side} temperature has no positive finite numerical center"
                    ),
                    "Store a positive finite exact or explicitly approximate temperature.",
                )
            })
    };
    let source_temperature = match numerical_temperature("source", source_context.temperature) {
        Ok(value) => value,
        Err(problem) => return not_evaluated(problem, None, None, None, None, None),
    };
    let target_temperature = match numerical_temperature("target", target_context.temperature) {
        Ok(value) => value,
        Err(problem) => return not_evaluated(problem, None, None, None, None, None),
    };
    let tolerance = settings.tolerance.to_f64_center().unwrap_or(f64::NAN);
    let grid_step = settings.grid_step.to_f64_center().unwrap_or(f64::NAN);
    if !tolerance.is_finite() || tolerance < 0.0 || !grid_step.is_finite() || grid_step <= 0.0 {
        return not_evaluated(
            refusal(
                "QA-COMPARISON-BOUNDARY",
                ContractStage::Admission,
                "comparison-boundary",
                "the tolerance or grid step has no admissible finite numerical center",
                "Store a nonnegative tolerance and a strictly positive grid step as typed scalars.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }
    for (coordinate, value, remedy) in [
        (
            "answer-type",
            settings.answer_sort.trim(),
            "Declare the response type produced by the query.",
        ),
        (
            "scope",
            settings.scope.trim(),
            "Declare the exact support over which the judgment ranges.",
        ),
        (
            "transformation",
            settings.transformation.trim(),
            "Declare the transformation from source analysis to target analysis.",
        ),
        (
            "transport",
            settings.transport.trim(),
            "Declare an identity, rename, or mass-preserving fusion transport.",
        ),
        (
            "scientific-layer.source",
            settings.source_layer.trim(),
            "Name the source scientific layer.",
        ),
        (
            "scientific-layer.target",
            settings.target_layer.trim(),
            "Name the target scientific layer.",
        ),
    ] {
        if value.is_empty() {
            return not_evaluated(
                refusal(
                    "QF-MISSING-DEPENDENCY",
                    ContractStage::Formation,
                    coordinate,
                    format!("required contract coordinate `{coordinate}` is empty"),
                    remedy,
                ),
                None,
                None,
                None,
                None,
                None,
            );
        }
    }
    if settings.layer_transport.trim().is_empty() {
        return not_evaluated(
            refusal(
                "QA-SCIENTIFIC-LAYER",
                ContractStage::Admission,
                "scientific-layer.transport",
                "the scientific-layer transport is empty",
                "Declare same-layer identity, or implement an executable typed bridge before requesting a cross-layer comparison.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }
    if settings.source_layer.trim() != settings.target_layer.trim() {
        return not_evaluated(
            refusal(
                "QA-SCIENTIFIC-LAYER-BRIDGE",
                ContractStage::Admission,
                "scientific-layer.transport",
                format!(
                    "declared layer transport `{}` is only an uninterpreted label: the engine has no executable typed bridge from `{}` to `{}`",
                    settings.layer_transport.trim(),
                    settings.source_layer.trim(),
                    settings.target_layer.trim()
                ),
                "Compare within one scientific layer, or implement and register a typed executable bridge with the declared source and target layers.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }
    if settings.query == QueryKind::ProbabilityLaw {
        for (side, context) in [("source", source_context), ("target", target_context)] {
            if context.kind != EvaluatorKind::MaxEnt {
                return not_evaluated(
                    refusal(
                        "QA-EVALUATOR-QUERY",
                        ContractStage::Admission,
                        &format!("{side}.evaluator"),
                        format!(
                            "a candidate probability law is undefined for the {side} {} evaluator",
                            context.kind.label()
                        ),
                        "Use MaxEnt on both sides or register a common deterministic response query.",
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
        }
    }
    if settings.response_domain == ResponseDomain::Trajectory {
        for (side, context) in [("source", source_context), ("target", target_context)] {
            if context.kind == EvaluatorKind::MaxEnt {
                return not_evaluated(
                    refusal(
                        "QA-SERIAL-EVALUATOR",
                        ContractStage::Admission,
                        &format!("{side}.evaluator"),
                        format!(
                            "a deterministic serial trajectory is not defined by the {side} MaxEnt evaluator without a sampling law"
                        ),
                        "Use OT/HG trajectories or register a stochastic transition and sampling law.",
                    ),
                    None,
                    None,
                    None,
                    None,
                    None,
                );
            }
        }
    }
    if settings.comparison_mode == ComparisonMode::Grid
        && (settings.response_domain == ResponseDomain::Trajectory
            || settings.query != QueryKind::ProbabilityLaw)
    {
        return not_evaluated(
            refusal(
                "QA-GRID-DOMAIN",
                ContractStage::Admission,
                "comparison-mode",
                "grid agreement is only defined here for numeric probability-law responses",
                "Use exact comparison for discrete responses or query a MaxEnt probability law.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }
    if settings.normalizer_policy == NormalizerPolicy::SharedDeclared
        && (source_context.kind != EvaluatorKind::MaxEnt
            || target_context.kind != EvaluatorKind::MaxEnt
            || settings.response_domain == ResponseDomain::Trajectory)
    {
        return not_evaluated(
            refusal(
                "QA-NORMALIZER-DOMAIN",
                ContractStage::Admission,
                "normalizer-policy",
                "a shared MaxEnt normalizer was declared for a response without one global MaxEnt partition function",
                "Use independent normalizers or a terminal MaxEnt probability-law comparison.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }
    let plan = match parse_transport(&settings.transport) {
        Ok(plan) => plan,
        Err(problem) => return not_evaluated(problem, None, None, None, None, None),
    };
    if plan.fusion && settings.query != QueryKind::ProbabilityLaw {
        return not_evaluated(
            refusal(
                "QA-FUSION-QUERY",
                ContractStage::Admission,
                "transport.answer-type",
                "a mass-preserving fusion requires a probability-law response",
                "Select the candidate probability law query or replace the fusion with an injective rename.",
            ),
            None,
            None,
            None,
            None,
            None,
        );
    }

    let (source_answer, target_answer) = match settings.response_domain {
        ResponseDomain::Terminal => (
            terminal_answer(
                source,
                source_context.kind,
                settings.query,
                source_temperature,
            ),
            terminal_answer(
                target,
                target_context.kind,
                settings.query,
                target_temperature,
            ),
        ),
        ResponseDomain::Trajectory => {
            let source_answer = match trajectory_answer(
                &source.constraints,
                source_serial,
                source_context.kind,
                source_temperature,
                "response.source-trajectory",
            ) {
                Ok(answer) => answer,
                Err(problem) => return not_evaluated(problem, None, None, None, None, None),
            };
            let target_answer = match trajectory_answer(
                &target.constraints,
                target_serial,
                target_context.kind,
                target_temperature,
                "response.target-trajectory",
            ) {
                Ok(answer) => answer,
                Err(problem) => {
                    return not_evaluated(problem, Some(&source_answer), None, None, None, None);
                }
            };
            (source_answer, target_answer)
        }
    };
    let transported_source = match apply_transport(&source_answer, &plan) {
        Ok(answer) => answer,
        Err(problem) => {
            return not_evaluated(
                problem,
                Some(&source_answer),
                None,
                Some(&target_answer),
                None,
                None,
            );
        }
    };

    let source_log_normalizer = (source_context.kind == EvaluatorKind::MaxEnt
        && settings.response_domain == ResponseDomain::Terminal)
        .then(|| maxent_log_normalizer(source, source_temperature));
    let target_log_normalizer = (target_context.kind == EvaluatorKind::MaxEnt
        && settings.response_domain == ResponseDomain::Terminal)
        .then(|| maxent_log_normalizer(target, target_temperature));
    let source_normalizer = source_log_normalizer.map(|value| format!("log Z = {value:.12}"));
    let target_normalizer = target_log_normalizer.map(|value| format!("log Z = {value:.12}"));

    let exact_source = if settings.query == QueryKind::ProbabilityLaw
        && settings.response_domain == ResponseDomain::Terminal
        && settings.comparison_mode == ComparisonMode::Exact
    {
        match exact_probability_law(source, source_context.temperature) {
            Ok(law) => Some(transport_exact_law(&law, &plan)),
            Err(problem) => {
                return not_evaluated(
                    problem,
                    Some(&source_answer),
                    Some(&transported_source),
                    Some(&target_answer),
                    source_normalizer,
                    target_normalizer,
                );
            }
        }
    } else {
        None
    };
    let exact_target = if exact_source.is_some() {
        match exact_probability_law(target, target_context.temperature) {
            Ok(law) => Some(law),
            Err(problem) => {
                return not_evaluated(
                    problem,
                    Some(&source_answer),
                    Some(&transported_source),
                    Some(&target_answer),
                    source_normalizer,
                    target_normalizer,
                );
            }
        }
    } else {
        None
    };

    if settings.normalizer_policy == NormalizerPolicy::SharedDeclared {
        let shared = match settings.comparison_mode {
            ComparisonMode::Exact => exact_source
                .as_ref()
                .zip(exact_target.as_ref())
                .is_some_and(|(left, right)| left.normalizer == right.normalizer),
            ComparisonMode::Approximate => source_log_normalizer
                .zip(target_log_normalizer)
                .is_some_and(|(left, right)| (left - right).abs() <= tolerance),
            ComparisonMode::Grid => source_log_normalizer
                .zip(target_log_normalizer)
                .is_some_and(|(left, right)| {
                    (left / grid_step).round() == (right / grid_step).round()
                }),
        };
        if !shared {
            return not_evaluated(
                refusal(
                    "QA-NORMALIZER-MISMATCH",
                    ContractStage::Admission,
                    "normalizer.shared",
                    format!(
                        "the independently calculated source and target normalizers do not satisfy the declared shared-normalizer policy ({}; {})",
                        source_normalizer.as_deref().unwrap_or("source unavailable"),
                        target_normalizer.as_deref().unwrap_or("target unavailable")
                    ),
                    "Use independent normalizers or revise the analyses so the declared shared partition function is mathematically valid.",
                ),
                Some(&source_answer),
                Some(&transported_source),
                Some(&target_answer),
                source_normalizer,
                target_normalizer,
            );
        }
    }

    let (compared_source, compared_target) =
        if settings.consumer_mode == ConsumerMode::LaterConsumer {
            let left = match apply_consumer(
                &transported_source,
                &settings.consumer,
                settings.response_domain,
                exact_source.is_some(),
            ) {
                Ok(answer) => answer,
                Err(problem) => {
                    return not_evaluated(
                        problem,
                        Some(&source_answer),
                        Some(&transported_source),
                        Some(&target_answer),
                        source_normalizer,
                        target_normalizer,
                    );
                }
            };
            let right = match apply_consumer(
                &target_answer,
                &settings.consumer,
                settings.response_domain,
                exact_target.is_some(),
            ) {
                Ok(answer) => answer,
                Err(problem) => {
                    return not_evaluated(
                        problem,
                        Some(&source_answer),
                        Some(&transported_source),
                        Some(&target_answer),
                        source_normalizer,
                        target_normalizer,
                    );
                }
            };
            (left, right)
        } else {
            (transported_source.clone(), target_answer.clone())
        };

    let discrepancies = match (&compared_source, &compared_target) {
        (ComputedAnswer::Discrete(left), ComputedAnswer::Discrete(right)) => {
            discrete_discrepancies(left, right)
        }
        (ComputedAnswer::Probability(left), ComputedAnswer::Probability(right)) => {
            match settings.comparison_mode {
                ComparisonMode::Exact => exact_probability_discrepancies(
                    exact_source.as_ref().expect("exact source law"),
                    exact_target.as_ref().expect("exact target law"),
                ),
                ComparisonMode::Approximate | ComparisonMode::Grid => {
                    approximate_probability_discrepancies(
                        left,
                        right,
                        settings.comparison_mode,
                        tolerance,
                        grid_step,
                    )
                }
            }
        }
        _ => {
            return not_evaluated(
                refusal(
                    "QA-ANSWER-TYPE",
                    ContractStage::Admission,
                    "answer-type",
                    "source and target consumers produced different response types",
                    "Use one consumer whose codomain is common to both independently computed responses.",
                ),
                Some(&source_answer),
                Some(&transported_source),
                Some(&target_answer),
                source_normalizer,
                target_normalizer,
            );
        }
    };
    let status = if discrepancies.is_empty() {
        ComparisonStatus::Preserved
    } else {
        ComparisonStatus::Discrepant
    };
    let certificate = (status == ComparisonStatus::Preserved).then(|| PreservationCertificate {
        statement: format!(
            "The target preserves the declared {} response under {} comparison.",
            settings.query.label(),
            settings.comparison_mode.label().to_ascii_lowercase()
        ),
        evidence: vec![
            "source and target responses were evaluated independently".to_owned(),
            format!(
                "source evaluator: {}; temperature: {}",
                source_context.kind.label(),
                source_context.temperature.canonical()
            ),
            format!(
                "target evaluator: {}; temperature: {}",
                target_context.kind.label(),
                target_context.temperature.canonical()
            ),
            format!("transport admitted: {}", settings.transport.trim()),
            format!("response domain: {}", settings.response_domain.label()),
            format!("normalizer policy: {}", settings.normalizer_policy.label()),
            format!(
                "scientific layer: {} → {}",
                settings.source_layer, settings.target_layer
            ),
            match settings.comparison_mode {
                ComparisonMode::Exact if settings.query == QueryKind::ProbabilityLaw => {
                    "exact normalized exponential-polynomial cross products matched".to_owned()
                }
                ComparisonMode::Exact => "exact discrete responses matched".to_owned(),
                ComparisonMode::Approximate => {
                    format!("absolute tolerance: {}", settings.tolerance.canonical())
                }
                ComparisonMode::Grid => {
                    format!("probability grid step: {}", settings.grid_step.canonical())
                }
            },
            if settings.consumer_mode == ConsumerMode::Direct {
                "comparison is direct".to_owned()
            } else {
                format!("later consumer: {}", settings.consumer.trim())
            },
        ],
    });
    SecondOrderResult {
        status,
        source_answer: source_answer.rendered(),
        transported_source_answer: transported_source.rendered(),
        target_answer: target_answer.rendered(),
        discrepancies,
        refusal: None,
        certificate,
        source_normalizer,
        target_normalizer,
    }
}

/// Compatibility wrapper for callers that want a direct, exact, terminal
/// identity comparison. New code should call `compare_tableaux_with_contract`.
#[cfg(test)]
pub(crate) fn compare_tableaux(
    source: &Tableau,
    target: &Tableau,
    kind: EvaluatorKind,
    query: QueryKind,
    temperature: &NumericScalar,
) -> SecondOrderResult {
    let mut settings = SecondOrderSettings {
        query,
        ..SecondOrderSettings::default()
    };
    settings.answer_sort = query.label().to_owned();
    compare_tableaux_with_contract(
        source,
        target,
        &SerialSettings {
            start: String::new(),
            moves: Vec::new(),
            maximum_steps: 64,
        },
        &SerialSettings {
            start: String::new(),
            moves: Vec::new(),
            maximum_steps: 64,
        },
        kind,
        temperature,
        &settings,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct SerialResult {
    pub path: Vec<String>,
    pub operations: Vec<String>,
    pub stopped: String,
    pub formed: bool,
}

pub(crate) fn evaluate_serial(
    constraints: &[crate::model::Constraint],
    moves: &[SerialMove],
    start: &str,
    kind: EvaluatorKind,
    temperature: f64,
    max_steps: usize,
) -> SerialResult {
    if kind == EvaluatorKind::MaxEnt {
        return SerialResult {
            path: vec![start.to_owned()],
            operations: Vec::new(),
            stopped: "not formed: deterministic serial selection requires OT or HG".to_owned(),
            formed: false,
        };
    }
    let mut current = start.to_owned();
    let mut path = vec![current.clone()];
    let mut operations = Vec::new();
    let mut seen = HashSet::from([current.clone()]);
    for _ in 0..max_steps {
        let eligible: Vec<&SerialMove> = moves.iter().filter(|item| item.from == current).collect();
        if eligible.is_empty() {
            return SerialResult {
                path,
                operations,
                stopped: "refused: no candidate set for the current form".to_owned(),
                formed: false,
            };
        }
        let tableau = Tableau {
            id: format!("serial-{current}"),
            name: current.clone(),
            input: current.clone(),
            constraints: constraints.to_vec(),
            candidates: eligible
                .iter()
                .enumerate()
                .map(|(index, item)| Candidate {
                    id: format!("serial-candidate-{index}"),
                    name: item.to.clone(),
                    form: item.to.clone(),
                    violations: item.violations.clone(),
                    base_mass: NumericScalar::integer(1),
                    notes: item.operation.clone(),
                    observed_frequency: NumericScalar::integer(0),
                    structured: None,
                })
                .collect(),
            tie_policy: "retain all co-winners".to_owned(),
            notes: String::new(),
            evaluator: None,
            temperature: None,
            missing_dependencies: Vec::new(),
            expected_winners: Vec::new(),
            source_locator: String::new(),
        };
        let result = evaluate(&tableau, kind, temperature);
        if result.winner_indices.len() != 1 {
            return SerialResult {
                path,
                operations,
                stopped: "refused: the local candidate set has co-winners".to_owned(),
                formed: false,
            };
        }
        let next = tableau.candidates[result.winner_indices[0]].name.clone();
        let operation = eligible[result.winner_indices[0]].operation.clone();
        if next == current {
            return SerialResult {
                path,
                operations,
                stopped: "faithful convergence".to_owned(),
                formed: true,
            };
        }
        operations.push(operation);
        path.push(next.clone());
        if !seen.insert(next.clone()) {
            return SerialResult {
                path,
                operations,
                stopped: "refused: cycle detected".to_owned(),
                formed: false,
            };
        }
        current = next;
    }
    SerialResult {
        path,
        operations,
        stopped: "refused: declared step limit reached".to_owned(),
        formed: false,
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RankingState {
    chosen: u64,
    survivors: Box<[u64]>,
}

/// Maximum number of distinct dynamic-programming states admitted by one
/// ranking-space calculation.
///
/// Exact ranking counts use arbitrary-precision integers, but exactness does
/// not make an exponentially large state graph free. The budget is charged on
/// cache misses across both the response-state and completion-count programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankingSpaceBudget {
    maximum_states: usize,
}

impl RankingSpaceBudget {
    pub fn new(maximum_states: usize) -> Result<Self, String> {
        if maximum_states == 0 {
            return Err("the Q-Calculus state budget must be strictly positive".to_owned());
        }
        Ok(Self { maximum_states })
    }

    pub const fn maximum_states(self) -> usize {
        self.maximum_states
    }
}

impl Default for RankingSpaceBudget {
    fn default() -> Self {
        Self {
            maximum_states: 2_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankingSpaceErrorKind {
    InvalidInput,
    StateBudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankingSpaceError {
    pub kind: RankingSpaceErrorKind,
    pub message: String,
}

impl RankingSpaceError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: RankingSpaceErrorKind::InvalidInput,
            message: message.into(),
        }
    }

    fn budget(limit: usize) -> Self {
        Self {
            kind: RankingSpaceErrorKind::StateBudgetExceeded,
            message: format!(
                "exact ranking-space evaluation reached the declared {limit}-state budget"
            ),
        }
    }
}

impl std::fmt::Display for RankingSpaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Debug, Default)]
struct RankingSpaceMeter {
    charged_states: usize,
    dynamic_states: usize,
    completion_states: usize,
}

impl RankingSpaceMeter {
    fn charge_dynamic(&mut self, budget: RankingSpaceBudget) -> Result<(), RankingSpaceError> {
        self.charge(budget)?;
        self.dynamic_states += 1;
        Ok(())
    }

    fn charge_completion(&mut self, budget: RankingSpaceBudget) -> Result<(), RankingSpaceError> {
        self.charge(budget)?;
        self.completion_states += 1;
        Ok(())
    }

    fn charge(&mut self, budget: RankingSpaceBudget) -> Result<(), RankingSpaceError> {
        if self.charged_states >= budget.maximum_states() {
            return Err(RankingSpaceError::budget(budget.maximum_states()));
        }
        self.charged_states += 1;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankingSpaceResult {
    pub total_rankings: BigUint,
    pub winner_counts: BTreeMap<Vec<String>, BigUint>,
    pub dynamic_states: usize,
    pub completion_states: usize,
    pub state_budget: usize,
    pub elapsed: Duration,
}

/// One reduced exact share of the compatible strict-ranking space.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactRankingShare {
    numerator: BigUint,
    denominator: BigUint,
}

impl ExactRankingShare {
    fn new(numerator: BigUint, denominator: BigUint) -> Self {
        debug_assert!(!denominator.is_zero());
        let reduced = Ratio::new(numerator, denominator);
        Self {
            numerator: reduced.numer().clone(),
            denominator: reduced.denom().clone(),
        }
    }

    pub const fn numerator(&self) -> &BigUint {
        &self.numerator
    }

    pub const fn denominator(&self) -> &BigUint {
        &self.denominator
    }

    /// Decimal projection for plotting only; certificates retain the exact
    /// reduced numerator and denominator. Q-Calculus admits at most sixty
    /// constraints, so both terms are bounded by `60!` and are representable
    /// as finite `f64` values.
    pub fn to_f64(&self) -> f64 {
        Ratio::new_raw(self.numerator.clone(), self.denominator.clone())
            .to_f64()
            .expect("a bounded Q-Calculus share must have a finite decimal projection")
    }
}

impl std::fmt::Display for ExactRankingShare {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.numerator, self.denominator)
    }
}

fn candidate_positions(mut mask: u64) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let bit = mask & mask.wrapping_neg();
        mask ^= bit;
        Some(bit.trailing_zeros() as usize)
    })
}

fn advance(tableau: &Tableau, survivors: u64, constraint: usize) -> u64 {
    let optimum = candidate_positions(survivors)
        .map(|candidate| {
            resolved_violation(tableau, &tableau.candidates[candidate], constraint)
                .unwrap_or(u16::MAX)
        })
        .min()
        .unwrap_or(0);
    candidate_positions(survivors).fold(0, |kept, candidate| {
        if resolved_violation(tableau, &tableau.candidates[candidate], constraint)
            .unwrap_or(u16::MAX)
            == optimum
        {
            kept | (1_u64 << candidate)
        } else {
            kept
        }
    })
}

fn fixed(tableau: &Tableau, survivors: u64, chosen: u64, active: &[usize]) -> bool {
    let remaining: Vec<usize> = active
        .iter()
        .enumerate()
        .filter_map(|(slot, constraint)| (chosen & (1_u64 << slot) == 0).then_some(*constraint))
        .collect();
    let mut positions = candidate_positions(survivors);
    let Some(first) = positions.next() else {
        return true;
    };
    let profile: Vec<u16> = remaining
        .iter()
        .map(|constraint| {
            resolved_violation(tableau, &tableau.candidates[first], *constraint).unwrap_or(u16::MAX)
        })
        .collect();
    positions.all(|candidate| {
        remaining
            .iter()
            .map(|constraint| {
                resolved_violation(tableau, &tableau.candidates[candidate], *constraint)
                    .unwrap_or(u16::MAX)
            })
            .eq(profile.iter().copied())
    })
}

fn available(chosen: u64, predecessors: &[u64]) -> impl Iterator<Item = usize> + '_ {
    predecessors
        .iter()
        .enumerate()
        .filter_map(move |(index, required)| {
            let bit = 1_u64 << index;
            (chosen & bit == 0 && required & !chosen == 0).then_some(index)
        })
}

fn completion_count(
    chosen: u64,
    predecessors: &[u64],
    full: u64,
    memo: &mut HashMap<u64, BigUint>,
    meter: &mut RankingSpaceMeter,
    budget: RankingSpaceBudget,
) -> Result<BigUint, RankingSpaceError> {
    if chosen == full {
        return Ok(BigUint::one());
    }
    if let Some(count) = memo.get(&chosen) {
        return Ok(count.clone());
    }
    meter.charge_completion(budget)?;

    let available_constraints: Vec<usize> = available(chosen, predecessors).collect();
    let remaining = (full ^ chosen).count_ones() as usize;
    if available_constraints.len() == remaining {
        let count = (2..=remaining).fold(BigUint::one(), |value, factor| value * factor);
        memo.insert(chosen, count.clone());
        return Ok(count);
    }
    let mut count = BigUint::zero();
    for constraint in available_constraints {
        count += completion_count(
            chosen | (1_u64 << constraint),
            predecessors,
            full,
            memo,
            meter,
            budget,
        )?;
    }
    memo.insert(chosen, count.clone());
    Ok(count)
}

struct RankingCountProgram<'a> {
    tableaus: &'a [Tableau],
    predecessors: &'a [u64],
    active: &'a [usize],
    full: u64,
    budget: RankingSpaceBudget,
}

fn ranking_counts(
    state: RankingState,
    program: &RankingCountProgram<'_>,
    memo: &mut HashMap<RankingState, HashMap<Box<[u64]>, BigUint>>,
    completion_memo: &mut HashMap<u64, BigUint>,
    meter: &mut RankingSpaceMeter,
) -> Result<HashMap<Box<[u64]>, BigUint>, RankingSpaceError> {
    if let Some(result) = memo.get(&state) {
        return Ok(result.clone());
    }
    meter.charge_dynamic(program.budget)?;
    if state.chosen == program.full
        || program
            .tableaus
            .iter()
            .zip(state.survivors.iter())
            .all(|(tableau, survivors)| fixed(tableau, *survivors, state.chosen, program.active))
    {
        let count = completion_count(
            state.chosen,
            program.predecessors,
            program.full,
            completion_memo,
            meter,
            program.budget,
        )?;
        let answer = state.survivors.clone();
        let result = HashMap::from([(answer, count)]);
        memo.insert(state, result.clone());
        return Ok(result);
    }
    let mut result: HashMap<Box<[u64]>, BigUint> = HashMap::new();
    for constraint in available(state.chosen, program.predecessors) {
        let next = RankingState {
            chosen: state.chosen | (1_u64 << constraint),
            survivors: program
                .tableaus
                .iter()
                .zip(state.survivors.iter())
                .map(|(tableau, survivors)| {
                    advance(tableau, *survivors, program.active[constraint])
                })
                .collect(),
        };
        for (answer, count) in ranking_counts(next, program, memo, completion_memo, meter)? {
            *result.entry(answer).or_insert_with(BigUint::zero) += count;
        }
    }
    memo.insert(state, result.clone());
    Ok(result)
}

#[cfg(test)]
pub(crate) fn ranking_space(tableaus: &[Tableau]) -> Result<RankingSpaceResult, RankingSpaceError> {
    ranking_space_with_budget(tableaus, RankingSpaceBudget::default())
}

pub(crate) fn ranking_space_with_budget(
    tableaus: &[Tableau],
    budget: RankingSpaceBudget,
) -> Result<RankingSpaceResult, RankingSpaceError> {
    let started = Instant::now();
    let Some(first) = tableaus.first() else {
        return Err(RankingSpaceError::invalid(
            "at least one tableau is required",
        ));
    };
    if tableaus
        .iter()
        .any(|tableau| tableau.tie_policy_kind() != TiePolicy::RetainAll)
    {
        return Err(RankingSpaceError::invalid(
            "Q-Calculus ranking shares require the `retain all co-winners` response policy",
        ));
    }
    let register_width = first.constraints.len();
    let active: Vec<usize> = first
        .constraints
        .iter()
        .enumerate()
        .filter_map(|(index, constraint)| constraint.enabled.then_some(index))
        .collect();
    let width = active.len();
    if width == 0 || width > 60 {
        return Err(RankingSpaceError::invalid(
            "ranking space supports 1 through 60 enabled constraints",
        ));
    }
    if tableaus.iter().any(|tableau| {
        tableau.constraints != first.constraints
            || tableau.candidates.is_empty()
            || tableau.candidates.len() > 63
            || tableau
                .candidates
                .iter()
                .any(|candidate| candidate.violations.len() != register_width)
    }) {
        return Err(RankingSpaceError::invalid(
            "tableaux must be nonempty and exactly constraint-aligned",
        ));
    }
    let mut predecessors = vec![0_u64; width];
    for (lower, lower_index) in active.iter().enumerate() {
        let lower_constraint = &first.constraints[*lower_index];
        for (higher, higher_index) in active.iter().enumerate() {
            let higher_constraint = &first.constraints[*higher_index];
            if higher_constraint.stratum < lower_constraint.stratum {
                predecessors[lower] |= 1_u64 << higher;
            }
        }
    }
    let full = (1_u64 << width) - 1;
    let initial = RankingState {
        chosen: 0,
        survivors: tableaus
            .iter()
            .map(|tableau| (1_u64 << tableau.candidates.len()) - 1)
            .collect(),
    };
    let mut memo = HashMap::new();
    let mut completion_memo = HashMap::new();
    let mut meter = RankingSpaceMeter::default();
    let program = RankingCountProgram {
        tableaus,
        predecessors: &predecessors,
        active: &active,
        full,
        budget,
    };
    let raw = ranking_counts(
        initial,
        &program,
        &mut memo,
        &mut completion_memo,
        &mut meter,
    )?;
    let total_rankings = completion_count(
        0,
        &predecessors,
        full,
        &mut completion_memo,
        &mut meter,
        budget,
    )?;
    let mut winner_counts = BTreeMap::new();
    for (answer, count) in raw {
        let labels = tableaus
            .iter()
            .zip(answer.iter())
            .flat_map(|(tableau, survivors)| {
                candidate_positions(*survivors).map(|candidate| {
                    format!("{} → {}", tableau.input, tableau.candidates[candidate].name)
                })
            })
            .collect();
        *winner_counts.entry(labels).or_insert_with(BigUint::zero) += count;
    }
    debug_assert_eq!(
        winner_counts.values().sum::<BigUint>(),
        total_rankings,
        "every compatible strict ranking must contribute to exactly one response class"
    );
    Ok(RankingSpaceResult {
        total_rankings,
        winner_counts,
        dynamic_states: meter.dynamic_states,
        completion_states: meter.completion_states,
        state_budget: budget.maximum_states(),
        elapsed: started.elapsed(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareShift {
    pub answer: Vec<String>,
    pub before: ExactRankingShare,
    pub after: ExactRankingShare,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneAuditResult {
    pub support_conservative: bool,
    pub shares_conservative: bool,
    pub before: RankingSpaceResult,
    pub after: RankingSpaceResult,
    pub shifts: Vec<ShareShift>,
}

#[cfg(test)]
pub(crate) fn clone_audit(
    tableau: &Tableau,
    source_constraint: usize,
) -> Result<CloneAuditResult, RankingSpaceError> {
    clone_audit_with_budget(tableau, source_constraint, RankingSpaceBudget::default())
}

pub(crate) fn clone_audit_with_budget(
    tableau: &Tableau,
    source_constraint: usize,
    budget: RankingSpaceBudget,
) -> Result<CloneAuditResult, RankingSpaceError> {
    if source_constraint >= tableau.constraints.len() {
        return Err(RankingSpaceError::invalid(
            "select a declared constraint to clone",
        ));
    }
    let mut transformed = tableau.clone();
    let mut clone = transformed.constraints[source_constraint].clone();
    clone.id = crate::model::next_stable_id(
        "constraint",
        transformed
            .constraints
            .iter()
            .map(|constraint| constraint.id.as_str()),
    );
    clone.name = format!("{} clone", clone.name);
    transformed.constraints.push(clone);
    for candidate in &mut transformed.candidates {
        candidate
            .violations
            .push(candidate.violations[source_constraint]);
    }
    let before = ranking_space_with_budget(std::slice::from_ref(tableau), budget)?;
    let after = ranking_space_with_budget(std::slice::from_ref(&transformed), budget)?;
    let support_conservative = before.winner_counts.keys().eq(after.winner_counts.keys());
    let mut all_answers: HashSet<Vec<String>> = before.winner_counts.keys().cloned().collect();
    all_answers.extend(after.winner_counts.keys().cloned());
    let mut shifts: Vec<ShareShift> = all_answers
        .into_iter()
        .map(|answer| ShareShift {
            before: ExactRankingShare::new(
                before
                    .winner_counts
                    .get(&answer)
                    .cloned()
                    .unwrap_or_else(BigUint::zero),
                before.total_rankings.clone(),
            ),
            after: ExactRankingShare::new(
                after
                    .winner_counts
                    .get(&answer)
                    .cloned()
                    .unwrap_or_else(BigUint::zero),
                after.total_rankings.clone(),
            ),
            answer,
        })
        .collect();
    shifts.sort_by(|left, right| left.answer.cmp(&right.answer));
    let shares_conservative = shifts.iter().all(|shift| shift.before == shift.after);
    Ok(CloneAuditResult {
        support_conservative,
        shares_conservative,
        before,
        after,
        shifts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_cases;
    use proptest::prelude::*;

    fn exact_scalar(literal: &str) -> NumericScalar {
        NumericScalar::parse_exact(literal).expect("test scalar is exact")
    }

    fn candidate(name: &str, violations: Vec<u16>, base_mass: i64) -> Candidate {
        Candidate {
            id: format!("candidate-{name}"),
            name: name.to_owned(),
            form: name.to_owned(),
            violations,
            base_mass: NumericScalar::integer(base_mass),
            notes: String::new(),
            observed_frequency: NumericScalar::integer(0),
            structured: None,
        }
    }

    #[test]
    fn prince_smolensky_ot_tableau_replicates_the_winner_and_fatal_mark() {
        let document = reference_cases::prince_smolensky_ot();
        let result = evaluate(&document.source, EvaluatorKind::Ot, 1.0);
        assert_eq!(result.winner_indices, [0]);
        assert_eq!(result.rows[1].fatal_constraint, Some(0));
    }

    #[test]
    fn tie_policies_preserve_the_native_tie_and_apply_declared_resolution() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.source.candidates[0].violations[0] = 0;
        document
            .source
            .candidates
            .push(candidate("candidate 2", vec![0], 1));

        document.source.set_tie_policy(TiePolicy::RetainAll);
        let retained = evaluate(&document.source, EvaluatorKind::Ot, 1.0);
        assert_eq!(retained.native_winner_indices, [0, 1]);
        assert_eq!(retained.winner_indices, [0, 1]);
        assert!(!retained.tie_unresolved);

        document.source.set_tie_policy(TiePolicy::FirstListed);
        let first = evaluate(&document.source, EvaluatorKind::Ot, 1.0);
        assert_eq!(first.native_winner_indices, [0, 1]);
        assert_eq!(first.winner_indices, [0]);

        document.source.set_tie_policy(TiePolicy::RequireUnique);
        let unique = evaluate(&document.source, EvaluatorKind::Ot, 1.0);
        assert_eq!(unique.native_winner_indices, [0, 1]);
        assert!(unique.winner_indices.is_empty());
        assert!(unique.tie_unresolved);
    }

    #[test]
    fn pater_hg_tableau_replicates_the_weighted_costs() {
        let document = reference_cases::pater_hg();
        let result = evaluate(&document.source, EvaluatorKind::HarmonicGrammar, 1.0);
        assert_eq!(result.winner_indices, [1]);
        assert!((result.rows[0].harmony - 1.5).abs() < 1e-12);
        assert!((result.rows[1].harmony - 1.0).abs() < 1e-12);
        assert_eq!(
            result.rows[0]
                .exact_harmony
                .as_ref()
                .map(ToString::to_string),
            Some("3/2".to_owned())
        );
        assert_eq!(
            result.rows[1]
                .exact_harmony
                .as_ref()
                .map(ToString::to_string),
            Some("1".to_owned())
        );
    }

    #[test]
    fn approximate_weight_keeps_harmony_explicitly_approximate() {
        let mut document = reference_cases::pater_hg();
        document.source.constraints[0].weight = Some(
            NumericScalar::gui_approximate(1.5)
                .expect("finite GUI value has approximation metadata"),
        );
        let result = evaluate(&document.source, EvaluatorKind::HarmonicGrammar, 1.0);
        assert!(result.rows.iter().all(|row| row.exact_harmony.is_none()));
        assert!(result.rows.iter().all(|row| row.harmony.is_finite()));
    }

    #[test]
    fn finite_maxent_smoke_case_is_normalized_and_uses_input_specific_z() {
        let document = reference_cases::finite_maxent_smoke();
        let result = evaluate(&document.source, EvaluatorKind::MaxEnt, 1.0);
        let mass: f64 = result.rows.iter().filter_map(|row| row.probability).sum();
        assert!((mass - 1.0).abs() < 1e-12);
        assert!(result.rows.iter().all(|row| row.probability.is_some()));
    }

    #[test]
    fn tiny_positive_temperature_is_used_by_the_maxent_normalizer() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.source.constraints[0].weight =
            Some(NumericScalar::parse_exact("0.000000001").expect("exact positive weight"));
        for constraint in document.source.constraints.iter_mut().skip(1) {
            constraint.enabled = false;
        }

        let observed = maxent_log_normalizer(&document.source, 1.0e-9);
        let expected = (1.0 + (-1.0_f64).exp()).ln();

        assert!((observed - expected).abs() < 1.0e-12);
    }

    #[test]
    fn finnish_clone_keeps_support_and_changes_shares() {
        let document = reference_cases::finnish_ranking_space();
        let result = clone_audit(&document.source, 0).expect("formed audit");
        assert!(result.support_conservative);
        assert!(!result.shares_conservative);
        assert_eq!(
            (
                result.before.total_rankings.to_string(),
                result.after.total_rankings.to_string()
            ),
            ("6".to_owned(), "24".to_owned())
        );
        assert!(
            result
                .shifts
                .iter()
                .any(|shift| shift.before.to_string() == "2/3" && shift.after.to_string() == "3/4")
        );
    }

    #[test]
    fn observationally_neutral_35_constraint_space_is_exact_beyond_u128() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.source.constraints = (0..35)
            .map(|index| crate::model::Constraint {
                id: format!("constraint-{index}"),
                name: format!("C{index}"),
                weight: Some(NumericScalar::integer(1)),
                stratum: 0,
                enabled: true,
                definition: String::new(),
                prior_mean: NumericScalar::integer(0),
                prior_sigma: NumericScalar::integer(100_000),
            })
            .collect();
        document.source.candidates = vec![candidate("only", vec![0; 35], 1)];

        let result = ranking_space(&[document.source]).expect("35! is an admitted exact count");
        let expected = (2_u32..=35).fold(BigUint::one(), |value, factor| value * factor);
        assert_eq!(result.total_rankings, expected);
        assert!(result.total_rankings > BigUint::from(u128::MAX));
        assert_eq!(result.dynamic_states, 1);
        assert_eq!(result.completion_states, 1);
    }

    #[test]
    fn ranking_space_ignores_disabled_constraints_everywhere() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.source.constraints = (0..3)
            .map(|index| crate::model::Constraint {
                id: format!("constraint-{index}"),
                name: format!("C{index}"),
                weight: Some(NumericScalar::integer(1)),
                stratum: 0,
                enabled: index != 1,
                definition: String::new(),
                prior_mean: NumericScalar::integer(0),
                prior_sigma: NumericScalar::integer(100_000),
            })
            .collect();
        document.source.candidates = vec![
            candidate("a", vec![0, 99, 1], 1),
            candidate("b", vec![1, 0, 0], 1),
        ];

        let result = ranking_space(&[document.source]).expect("two enabled constraints form");
        assert_eq!(result.total_rankings, BigUint::from(2_u8));
        assert_eq!(result.winner_counts.len(), 2);
        assert!(
            result
                .winner_counts
                .values()
                .all(|count| count == &BigUint::one())
        );
    }

    #[test]
    fn finite_gen1_serial_case_converges_with_identity_candidate() {
        let document = reference_cases::serial_syllabification_smoke();
        let result = evaluate_serial(
            &document.source.constraints,
            &document.serial.moves,
            &document.serial.start,
            EvaluatorKind::Ot,
            1.0,
            20,
        );
        assert_eq!(result.stopped, "faithful convergence");
        assert!(result.formed);
        assert_eq!(result.path, ["txznt", "tx(zN)t"]);
    }

    #[test]
    fn serial_cycles_are_refused() {
        let constraints = reference_cases::serial_syllabification_smoke()
            .source
            .constraints;
        let cycling = vec![
            SerialMove {
                from: "a".to_owned(),
                to: "b".to_owned(),
                operation: "change".to_owned(),
                violations: vec![0, 0],
            },
            SerialMove {
                from: "b".to_owned(),
                to: "a".to_owned(),
                operation: "reverse".to_owned(),
                violations: vec![0, 0],
            },
        ];
        let cycle = evaluate_serial(&constraints, &cycling, "a", EvaluatorKind::Ot, 1.0, 20);
        assert_eq!(cycle.stopped, "refused: cycle detected");
        assert!(!cycle.formed);
        assert_eq!(cycle.path, ["a", "b", "a"]);
    }

    #[test]
    fn dissertation_second_order_case_preserves_winner_but_reverses_order() {
        let document = reference_cases::dissertation_second_order();
        let winner = compare_tableaux(
            &document.source,
            &document.target,
            document.evaluator,
            QueryKind::WinnerSet,
            &document.temperature,
        );
        let order = compare_tableaux(
            &document.source,
            &document.target,
            document.evaluator,
            QueryKind::CompleteOrder,
            &document.temperature,
        );
        assert!(winner.conservative());
        assert!(!order.conservative());
    }

    #[test]
    fn discrepancy_and_unformed_comparison_are_distinct_results() {
        let mut document = reference_cases::dissertation_second_order();
        let discrepancy = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(discrepancy.status, ComparisonStatus::Discrepant);
        assert!(!discrepancy.discrepancies.is_empty());
        assert!(discrepancy.refusal.is_none());

        document.second_order.transport.clear();
        let unformed = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(unformed.status, ComparisonStatus::NotEvaluated);
        assert_eq!(
            unformed
                .refusal
                .as_ref()
                .map(|item| item.coordinate.as_str()),
            Some("transport")
        );
        assert!(unformed.discrepancies.is_empty());
    }

    #[test]
    fn candidate_renaming_aligns_independently_calculated_winners() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.source.candidates[0].name = "source-winner".to_owned();
        document.target.candidates[0].name = "target-winner".to_owned();
        document.second_order.transport = "rename source-winner=target-winner".to_owned();
        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(result.status, ComparisonStatus::Preserved);
        assert_eq!(
            result.transported_source_answer,
            vec![vec!["target-winner".to_owned()]]
        );
        assert!(result.certificate.is_some());
    }

    #[test]
    fn exact_mass_preserving_fusion_is_certified_symbolically() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.evaluator = EvaluatorKind::MaxEnt;
        document.source.candidates = vec![candidate("a", vec![0], 1), candidate("b", vec![0], 1)];
        document.target.candidates = vec![candidate("c", vec![0], 2)];
        document.second_order.query = QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law on target support".to_owned();
        document.second_order.transport = "fusion a+b=c; mass-preserving".to_owned();
        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(result.status, ComparisonStatus::Preserved);
        assert!(
            result
                .certificate
                .as_ref()
                .is_some_and(|certificate| certificate.statement.contains("exact"))
        );
    }

    #[test]
    fn equal_maxent_laws_can_have_distinct_normalizers() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.evaluator = EvaluatorKind::MaxEnt;
        document.source.candidates = vec![candidate("a", vec![0], 1), candidate("b", vec![1], 1)];
        document.target.candidates = vec![candidate("a", vec![1], 1), candidate("b", vec![2], 1)];
        document.second_order.query = QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();
        let independent = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(independent.status, ComparisonStatus::Preserved);
        assert_ne!(independent.source_normalizer, independent.target_normalizer);

        document.second_order.normalizer_policy = NormalizerPolicy::SharedDeclared;
        let shared = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(shared.status, ComparisonStatus::NotEvaluated);
        assert_eq!(
            shared.refusal.as_ref().map(|item| item.code.as_str()),
            Some("QA-NORMALIZER-MISMATCH")
        );
    }

    #[test]
    fn terminal_and_trajectory_responses_have_independent_admission() {
        let mut document = reference_cases::serial_syllabification_smoke();
        document.target = document.source.clone();
        document.target_serial = document.serial.clone();
        document.second_order.response_domain = ResponseDomain::Trajectory;
        document.second_order.answer_sort = "ordered derivational trajectory".to_owned();
        let trajectory = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(trajectory.status, ComparisonStatus::Preserved);

        document.target_serial.moves.clear();
        let missing = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(missing.status, ComparisonStatus::NotEvaluated);
        assert_eq!(
            missing
                .refusal
                .as_ref()
                .map(|item| item.coordinate.as_str()),
            Some("response.target-trajectory")
        );
    }

    #[test]
    fn exact_approximate_and_grid_probability_judgments_do_not_collapse() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.evaluator = EvaluatorKind::MaxEnt;
        document.source.candidates[0].violations[0] = 0;
        document
            .source
            .candidates
            .push(candidate("candidate 2", vec![1], 1));
        document.target = document.source.clone();
        document.target.constraints[0].weight = Some(exact_scalar("1.0001"));
        document.second_order.query = QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();
        let exact = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(exact.status, ComparisonStatus::Discrepant);

        document.second_order.comparison_mode = ComparisonMode::Approximate;
        document.second_order.tolerance = exact_scalar("0.001");
        let approximate = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(approximate.status, ComparisonStatus::Preserved);

        document.second_order.comparison_mode = ComparisonMode::Grid;
        document.second_order.grid_step = exact_scalar("0.01");
        let grid = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(grid.status, ComparisonStatus::Preserved);
    }

    #[test]
    fn exact_probability_mode_refuses_explicitly_approximate_parameters() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.target = document.source.clone();
        document.target.constraints[0].weight = Some(
            NumericScalar::gui_approximate(1.0)
                .expect("finite approximation carries an explicit boundary"),
        );
        document.second_order.query = QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();
        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        let refusal = result.refusal.expect("exactness refusal is indexed");
        assert_eq!(refusal.code, "QC-APPROXIMATE-WEIGHT");
        assert_eq!(refusal.stage, ContractStage::Certification);
        assert!(result.certificate.is_none());
    }

    #[test]
    fn exact_certification_dependency_is_scoped_to_exact_mode() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.target = document.source.clone();
        document
            .source
            .missing_dependencies
            .push(crate::model::MissingDependency {
                code: "QC-MISSING-SYMBOLIC-CERTIFICATE".to_owned(),
                stage: DependencyStage::Admission,
                coordinate: "certificate.symbolic-fixture".to_owned(),
                scope: crate::model::DependencyScope::ExactCertification,
                message: "the symbolic fixture is unavailable".to_owned(),
                remedy: "supply the symbolic fixture or select approximate mode".to_owned(),
            });
        document.second_order.query = QueryKind::ProbabilityLaw;
        document.second_order.answer_sort = "probability law".to_owned();
        let exact_result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(exact_result.status, ComparisonStatus::NotEvaluated);
        let refusal = exact_result.refusal.expect("scope-specific refusal");
        assert_eq!(refusal.code, "QC-MISSING-SYMBOLIC-CERTIFICATE");
        assert_eq!(refusal.coordinate, "source.certificate.symbolic-fixture");

        document.second_order.comparison_mode = ComparisonMode::Approximate;
        let approximate_result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(approximate_result.status, ComparisonStatus::Preserved);
    }

    #[test]
    fn later_consumers_can_preserve_what_direct_comparison_does_not() {
        let mut document = reference_cases::dissertation_second_order();
        let direct = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(direct.status, ComparisonStatus::Discrepant);

        document.second_order.consumer_mode = ConsumerMode::LaterConsumer;
        document.second_order.consumer = "winner-set".to_owned();
        let consumed = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(consumed.status, ComparisonStatus::Preserved);

        document.second_order.consumer.clear();
        let missing = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(missing.status, ComparisonStatus::NotEvaluated);
        assert_eq!(
            missing
                .refusal
                .as_ref()
                .map(|item| item.coordinate.as_str()),
            Some("consumer")
        );
    }

    #[test]
    fn cross_layer_identity_is_refused_before_evaluation() {
        let mut document = crate::model::ConvalgenDocument::blank();
        document.second_order.source_layer = "grammar".to_owned();
        document.second_order.target_layer = "phonetic realization".to_owned();
        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );
        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        let refusal = result.refusal.expect("indexed cross-layer refusal");
        assert_eq!(refusal.code, "QA-SCIENTIFIC-LAYER-BRIDGE");
        assert_eq!(refusal.stage, ContractStage::Admission);
        assert_eq!(refusal.coordinate, "scientific-layer.transport");
        assert!(refusal.message.contains("no executable typed bridge"));
    }

    #[test]
    fn arbitrary_cross_layer_label_is_not_an_executable_bridge() {
        let mut document = reference_cases::dissertation_second_order();
        document.second_order.source_layer = "grammar".to_owned();
        document.second_order.target_layer = "phonetic realization".to_owned();
        document.second_order.layer_transport = "acoustic-observation-map".to_owned();

        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );

        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        let refusal = result.refusal.expect("indexed cross-layer refusal");
        assert_eq!(refusal.code, "QA-SCIENTIFIC-LAYER-BRIDGE");
        assert_eq!(refusal.stage, ContractStage::Admission);
        assert_eq!(refusal.coordinate, "scientific-layer.transport");
        assert!(refusal.message.contains("acoustic-observation-map"));
        assert!(refusal.message.contains("grammar"));
        assert!(refusal.message.contains("phonetic realization"));
    }

    #[test]
    fn same_layer_identity_reaches_the_declared_comparison() {
        let mut document = reference_cases::dissertation_second_order();
        document.second_order.source_layer = "grammar".to_owned();
        document.second_order.target_layer = "grammar".to_owned();
        document.second_order.layer_transport = "identity".to_owned();

        let result = compare_tableaux_with_contract(
            &document.source,
            &document.target,
            &document.serial,
            &document.target_serial,
            document.evaluator,
            &document.temperature,
            &document.second_order,
        );

        assert_eq!(result.status, ComparisonStatus::Discrepant);
        assert!(result.refusal.is_none());
    }

    #[test]
    fn maxent_softmax_remains_normalized_for_large_harmonies() {
        let mut document = reference_cases::finite_maxent_smoke();
        for constraint in &mut document.source.constraints {
            constraint.weight = constraint.weight.as_ref().map(|weight| {
                weight
                    .checked_mul(&NumericScalar::integer(100_000))
                    .expect("exact weight multiplication")
            });
        }
        let result = evaluate(&document.source, EvaluatorKind::MaxEnt, 0.01);
        let masses: Vec<f64> = result
            .rows
            .iter()
            .filter_map(|row| row.probability)
            .collect();
        assert!(masses.iter().all(|mass| mass.is_finite()));
        assert!((masses.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn constraint_definitions_never_override_analyst_supplied_marks() {
        let mut document = reference_cases::prince_smolensky_ot();
        document.source.input = "/abc/".to_owned();
        document.source.candidates[0].form = "abb".to_owned();
        document.source.candidates[0].violations[0] = 7;
        document.source.candidates[0].violations[1] = 11;
        document.source.constraints[0].definition = "Count every [b].".to_owned();
        document.source.constraints[1].definition = "Penalize edit distance.".to_owned();
        assert_eq!(
            resolved_violation(&document.source, &document.source.candidates[0], 0),
            Ok(7)
        );
        assert_eq!(
            resolved_violation(&document.source, &document.source.candidates[0], 1),
            Ok(11)
        );
    }

    proptest! {
        #[test]
        fn compiled_integer_weight_register_matches_the_exact_dot_product(
            terms in prop::collection::vec((0_i64..100, 0_u16..100), 1..24),
        ) {
            let mut document = crate::model::ConvalgenDocument::blank();
            document.source.constraints = terms
                .iter()
                .enumerate()
                .map(|(index, (weight, _))| crate::model::Constraint {
                    id: format!("constraint-{index}"),
                    name: format!("C{index}"),
                    weight: Some(NumericScalar::integer(*weight)),
                    stratum: index,
                    enabled: true,
                    definition: String::new(),
                    prior_mean: NumericScalar::integer(0),
                    prior_sigma: NumericScalar::integer(100_000),
                })
                .collect();
            document.source.candidates = vec![candidate(
                "only",
                terms.iter().map(|(_, violation)| *violation).collect(),
                1,
            )];
            let expected = terms.iter().fold(BigInt::zero(), |total, (weight, violation)| {
                total + BigInt::from(*weight) * BigInt::from(*violation)
            });
            let result = evaluate(&document.source, EvaluatorKind::MaxEnt, 1.0);
            prop_assert_eq!(
                result.rows[0].exact_harmony.as_ref(),
                Some(&BigRational::from_integer(expected)),
            );
            prop_assert_eq!(result.rows[0].probability, Some(1.0));
        }

        #[test]
        fn neutral_ranking_counts_equal_factorial_of_enabled_register(
            enabled in prop::collection::vec(any::<bool>(), 1..10),
        ) {
            prop_assume!(enabled.iter().any(|value| *value));
            let mut document = crate::model::ConvalgenDocument::blank();
            document.source.constraints = enabled
                .iter()
                .enumerate()
                .map(|(index, enabled)| crate::model::Constraint {
                    id: format!("constraint-{index}"),
                    name: format!("C{index}"),
                    weight: Some(NumericScalar::integer(1)),
                    stratum: 0,
                    enabled: *enabled,
                    definition: String::new(),
                    prior_mean: NumericScalar::integer(0),
                    prior_sigma: NumericScalar::integer(100_000),
                })
                .collect();
            document.source.candidates = vec![candidate("only", vec![0; enabled.len()], 1)];
            let active = enabled.iter().filter(|value| **value).count();
            let expected = (2..=active).fold(BigUint::one(), |value, factor| value * factor);
            let result = ranking_space(&[document.source]).expect("bounded neutral space forms");
            prop_assert_eq!(result.total_rankings, expected);
        }

        #[test]
        fn maxent_probability_laws_normalize_for_random_finite_tableaux(
            marks in prop::collection::vec(0_u16..50, 1..40),
            weight in 0.001_f64..100.0,
            temperature in 0.01_f64..20.0,
        ) {
            let mut document = crate::model::ConvalgenDocument::blank();
            document.source.constraints[0].weight = Some(
                NumericScalar::gui_approximate(weight)
                    .expect("proptest provides a finite approximate weight"),
            );
            document.source.candidates = marks
                .iter()
                .enumerate()
                .map(|(index, mark)| candidate(&format!("c{index}"), vec![*mark], 1))
                .collect();
            let result = evaluate(&document.source, EvaluatorKind::MaxEnt, temperature);
            let probabilities: Vec<f64> = result.rows.iter().filter_map(|row| row.probability).collect();
            prop_assert!(probabilities.iter().all(|value| value.is_finite() && *value >= 0.0));
            prop_assert!((probabilities.iter().sum::<f64>() - 1.0).abs() < 1.0e-10);
        }

        #[test]
        fn ot_complete_order_is_invariant_under_uniform_constraint_offsets(
            rows in prop::collection::vec(prop::array::uniform3(0_u16..20), 1..30),
            offsets in prop::array::uniform3(0_u16..20),
        ) {
            let mut document = crate::model::ConvalgenDocument::blank();
            document.source.constraints = vec![
                crate::model::Constraint { id: "constraint-1".to_owned(), name: "C1".to_owned(), weight: Some(NumericScalar::integer(1)), stratum: 0, enabled: true, definition: String::new(), prior_mean: NumericScalar::integer(0), prior_sigma: NumericScalar::integer(100_000) },
                crate::model::Constraint { id: "constraint-2".to_owned(), name: "C2".to_owned(), weight: Some(NumericScalar::integer(1)), stratum: 1, enabled: true, definition: String::new(), prior_mean: NumericScalar::integer(0), prior_sigma: NumericScalar::integer(100_000) },
                crate::model::Constraint { id: "constraint-3".to_owned(), name: "C3".to_owned(), weight: Some(NumericScalar::integer(1)), stratum: 2, enabled: true, definition: String::new(), prior_mean: NumericScalar::integer(0), prior_sigma: NumericScalar::integer(100_000) },
            ];
            document.source.candidates = rows
                .iter()
                .enumerate()
                .map(|(index, row)| candidate(&format!("c{index}"), row.to_vec(), 1))
                .collect();
            let before = query_answer(
                &document.source,
                &evaluate(&document.source, EvaluatorKind::Ot, 1.0),
                QueryKind::CompleteOrder,
            );
            for candidate in &mut document.source.candidates {
                for (mark, offset) in candidate.violations.iter_mut().zip(offsets) {
                    *mark = mark.saturating_add(offset);
                }
            }
            let after = query_answer(
                &document.source,
                &evaluate(&document.source, EvaluatorKind::Ot, 1.0),
                QueryKind::CompleteOrder,
            );
            prop_assert_eq!(before, after);
        }
    }
}
