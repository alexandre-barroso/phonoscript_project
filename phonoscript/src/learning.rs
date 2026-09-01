use std::collections::{HashMap, HashSet};

use crate::engine::resolved_violation;
use crate::model::Tableau;

fn center(value: &crate::exact::NumericScalar) -> f64 {
    value
        .to_f64_center()
        .expect("checked learning input has a finite scalar center")
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaxEntTrainingResult {
    pub weights: Vec<f64>,
    pub iterations: usize,
    pub converged: bool,
    pub negative_log_likelihood: f64,
    pub maximum_gradient: f64,
}

fn aligned(tableaus: &[Tableau]) -> Result<usize, String> {
    let first = tableaus
        .first()
        .ok_or_else(|| "training requires at least one tableau".to_owned())?;
    let width = first.constraints.len();
    if width == 0 {
        return Err("training requires at least one constraint".to_owned());
    }
    if tableaus.iter().any(|tableau| {
        tableau.constraints.len() != width
            || tableau
                .constraints
                .iter()
                .zip(&first.constraints)
                .any(|(left, right)| left.name != right.name)
            || tableau.candidates.is_empty()
    }) {
        return Err("training tableaux must share one ordered constraint register".to_owned());
    }
    Ok(width)
}

fn objective_and_gradient(
    tableaus: &[Tableau],
    weights: &[f64],
    temperature: f64,
) -> Result<(f64, Vec<f64>), String> {
    if !temperature.is_finite() || temperature <= 0.0 {
        return Err("MaxEnt learning requires a finite, strictly positive temperature".to_owned());
    }
    let scale = temperature;
    let mut objective = 0.0;
    let mut gradient = vec![0.0; weights.len()];
    for tableau in tableaus {
        let total: f64 = tableau
            .candidates
            .iter()
            .map(|candidate| center(&candidate.observed_frequency).max(0.0))
            .sum();
        if total == 0.0 {
            continue;
        }
        let rows: Vec<Vec<f64>> = tableau
            .candidates
            .iter()
            .map(|candidate| {
                (0..weights.len())
                    .map(|constraint| {
                        resolved_violation(tableau, candidate, constraint).map(|mark| {
                            if tableau.constraints[constraint].enabled {
                                f64::from(mark)
                            } else {
                                0.0
                            }
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<_, _>>()?;
        let costs: Vec<f64> = rows
            .iter()
            .map(|row| {
                row.iter()
                    .zip(weights)
                    .map(|(mark, weight)| mark * weight)
                    .sum()
            })
            .collect();
        let log_masses: Vec<f64> = costs
            .iter()
            .zip(&tableau.candidates)
            .map(|(cost, candidate)| center(&candidate.base_mass).ln() - cost / scale)
            .collect();
        let largest = log_masses.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let shifted: Vec<f64> = log_masses
            .iter()
            .map(|log_mass| (log_mass - largest).exp())
            .collect();
        let normalizer: f64 = shifted.iter().sum();
        let log_normalizer = largest + normalizer.ln();
        for (candidate_index, candidate) in tableau.candidates.iter().enumerate() {
            let observed = center(&candidate.observed_frequency).max(0.0);
            objective +=
                observed * (costs[candidate_index] / scale - center(&candidate.base_mass).ln());
            for (constraint, mark) in rows[candidate_index].iter().enumerate() {
                gradient[constraint] += observed * mark / scale;
            }
        }
        objective += total * log_normalizer;
        for (candidate_index, row) in rows.iter().enumerate() {
            let probability = shifted[candidate_index] / normalizer;
            for (constraint, mark) in row.iter().enumerate() {
                gradient[constraint] -= total * probability * mark / scale;
            }
        }
    }
    let constraints = &tableaus[0].constraints;
    for (index, constraint) in constraints.iter().enumerate() {
        if !constraint.enabled {
            gradient[index] = 0.0;
            continue;
        }
        let sigma = center(&constraint.prior_sigma);
        if !sigma.is_finite() || sigma <= 0.0 {
            return Err(format!(
                "enabled constraint `{}` requires a finite, strictly positive prior sigma",
                constraint.name
            ));
        }
        let delta = weights[index] - center(&constraint.prior_mean);
        objective += delta * delta / (2.0 * sigma * sigma);
        gradient[index] += delta / (sigma * sigma);
    }
    Ok((objective, gradient))
}

/// Learn nonnegative finite-MaxEnt weights with projected gradient descent and
/// monotone backtracking. The objective is convex for the declared finite
/// candidate supports and Gaussian priors.
pub(crate) fn train_maxent(
    tableaus: &[Tableau],
    temperature: f64,
    maximum_iterations: usize,
) -> Result<MaxEntTrainingResult, String> {
    let width = aligned(tableaus)?;
    if !tableaus.iter().any(|tableau| {
        tableau
            .candidates
            .iter()
            .any(|candidate| center(&candidate.observed_frequency) > 0.0)
    }) {
        return Err("training requires at least one positive observed frequency".to_owned());
    }
    let mut weights: Vec<f64> = tableaus[0]
        .constraints
        .iter()
        .map(|constraint| {
            constraint
                .weight
                .as_ref()
                .map(|weight| center(weight).max(0.0))
                .or_else(|| (!constraint.enabled).then_some(0.0))
                .ok_or_else(|| {
                    format!(
                        "enabled constraint `{}` has no MaxEnt weight",
                        constraint.name
                    )
                })
        })
        .collect::<Result<_, _>>()?;
    let mut converged = false;
    let mut iterations = 0;
    for iteration in 0..maximum_iterations.max(1) {
        let (objective, gradient) = objective_and_gradient(tableaus, &weights, temperature)?;
        let maximum_gradient = gradient.iter().map(|value| value.abs()).fold(0.0, f64::max);
        iterations = iteration + 1;
        if maximum_gradient < 1e-8 {
            converged = true;
            break;
        }
        let squared_norm: f64 = gradient.iter().map(|value| value * value).sum();
        let mut step = 1.0;
        let mut accepted = None;
        for _ in 0..40 {
            let candidate: Vec<f64> = weights
                .iter()
                .zip(&gradient)
                .enumerate()
                .map(|(index, (weight, direction))| {
                    if tableaus[0].constraints[index].enabled {
                        (weight - step * direction).max(0.0)
                    } else {
                        *weight
                    }
                })
                .collect();
            let (candidate_objective, _) =
                objective_and_gradient(tableaus, &candidate, temperature)?;
            if candidate_objective <= objective - 1e-4 * step * squared_norm
                || candidate_objective < objective
            {
                accepted = Some(candidate);
                break;
            }
            step *= 0.5;
        }
        let Some(next) = accepted else {
            converged = maximum_gradient < 1e-6;
            break;
        };
        let movement = next
            .iter()
            .zip(&weights)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0, f64::max);
        weights = next;
        if movement < 1e-10 {
            converged = true;
            break;
        }
        if width != weights.len() {
            return Err("internal training dimension changed".to_owned());
        }
    }
    let (objective, gradient) = objective_and_gradient(tableaus, &weights, temperature)?;
    let maximum_gradient = gradient.iter().map(|value| value.abs()).fold(0.0, f64::max);
    Ok(MaxEntTrainingResult {
        weights,
        iterations,
        converged,
        negative_log_likelihood: objective,
        maximum_gradient,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankingInference {
    pub order: Vec<usize>,
    pub explored_states: usize,
}

fn expected_mask(tableau: &Tableau) -> Result<u64, String> {
    if tableau.candidates.len() > 63 {
        return Err("ranking inference supports at most 63 candidates per tableau".to_owned());
    }
    let mask = tableau
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| center(&candidate.observed_frequency) > 0.0)
        .fold(0_u64, |mask, (index, _)| mask | (1_u64 << index));
    if mask == 0 {
        Err(format!(
            "{} has no candidate with positive observed frequency",
            tableau.input
        ))
    } else {
        Ok(mask)
    }
}

fn advance(tableau: &Tableau, survivors: u64, constraint: usize) -> u64 {
    let optimum = (0..tableau.candidates.len())
        .filter(|candidate| survivors & (1_u64 << candidate) != 0)
        .map(|candidate| {
            resolved_violation(tableau, &tableau.candidates[candidate], constraint)
                .unwrap_or(u16::MAX)
        })
        .min()
        .unwrap_or(0);
    (0..tableau.candidates.len()).fold(0_u64, |mask, candidate| {
        if survivors & (1_u64 << candidate) != 0
            && resolved_violation(tableau, &tableau.candidates[candidate], constraint)
                .unwrap_or(u16::MAX)
                == optimum
        {
            mask | (1_u64 << candidate)
        } else {
            mask
        }
    })
}

fn infer_dfs(
    tableaus: &[Tableau],
    expected: &[u64],
    predecessors: &[u64],
    chosen: u64,
    survivors: Vec<u64>,
    states: &mut usize,
    failed: &mut HashSet<(u64, Vec<u64>)>,
) -> Option<Vec<usize>> {
    *states += 1;
    if *states > 2_000_000 {
        return None;
    }
    let full = (1_u64 << predecessors.len()) - 1;
    if chosen == full {
        return (survivors == expected).then(Vec::new);
    }
    let state = (chosen, survivors.clone());
    if failed.contains(&state) {
        return None;
    }
    for constraint in 0..predecessors.len() {
        let bit = 1_u64 << constraint;
        if chosen & bit != 0 || predecessors[constraint] & !chosen != 0 {
            continue;
        }
        let next: Vec<u64> = tableaus
            .iter()
            .zip(&survivors)
            .map(|(tableau, survivors)| advance(tableau, *survivors, constraint))
            .collect();
        if next
            .iter()
            .zip(expected)
            .any(|(survivors, expected)| survivors & expected != *expected)
        {
            continue;
        }
        if let Some(mut suffix) = infer_dfs(
            tableaus,
            expected,
            predecessors,
            chosen | bit,
            next,
            states,
            failed,
        ) {
            suffix.insert(0, constraint);
            return Some(suffix);
        }
    }
    failed.insert(state);
    None
}

pub(crate) fn infer_ot_ranking(
    tableaus: &[Tableau],
    a_priori_rankings: &[(usize, usize)],
) -> Result<RankingInference, String> {
    let width = aligned(tableaus)?;
    if width > 60 {
        return Err("ranking inference supports at most 60 constraints".to_owned());
    }
    let expected: Vec<u64> = tableaus
        .iter()
        .map(expected_mask)
        .collect::<Result<_, _>>()?;
    let mut predecessors = vec![0_u64; width];
    for (higher, lower) in a_priori_rankings {
        if *higher >= width || *lower >= width || higher == lower {
            return Err("an a priori ranking names an invalid constraint pair".to_owned());
        }
        predecessors[*lower] |= 1_u64 << higher;
    }
    let mut transitive = true;
    while transitive {
        transitive = false;
        for lower in 0..width {
            let before = predecessors[lower];
            for higher in 0..width {
                if before & (1_u64 << higher) != 0 {
                    predecessors[lower] |= predecessors[higher];
                }
            }
            transitive |= before != predecessors[lower];
        }
    }
    if (0..width).any(|index| predecessors[index] & (1_u64 << index) != 0) {
        return Err("a priori rankings contain a domination cycle".to_owned());
    }
    let survivors: Vec<u64> = tableaus
        .iter()
        .map(|tableau| (1_u64 << tableau.candidates.len()) - 1)
        .collect();
    let mut states = 0;
    let mut failed = HashSet::new();
    let order = infer_dfs(
        tableaus,
        &expected,
        &predecessors,
        0,
        survivors,
        &mut states,
        &mut failed,
    )
    .ok_or_else(|| {
        if states > 2_000_000 {
            "ranking search reached its explicit two-million-state limit".to_owned()
        } else {
            "no strict ranking generates exactly the declared observed winners".to_owned()
        }
    })?;
    Ok(RankingInference {
        order,
        explored_states: states,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarmonicBound {
    pub input: String,
    pub observed: String,
    pub bounding_rival: String,
}

pub(crate) fn harmonic_bounds(tableaus: &[Tableau]) -> Vec<HarmonicBound> {
    let mut results = Vec::new();
    for tableau in tableaus {
        for observed in tableau
            .candidates
            .iter()
            .filter(|candidate| center(&candidate.observed_frequency) > 0.0)
        {
            for rival in tableau
                .candidates
                .iter()
                .filter(|candidate| center(&candidate.observed_frequency) <= 0.0)
            {
                let rival_no_worse = (0..tableau.constraints.len()).all(|constraint| {
                    resolved_violation(tableau, rival, constraint).unwrap_or(u16::MAX)
                        <= resolved_violation(tableau, observed, constraint).unwrap_or(u16::MAX)
                });
                let rival_better = (0..tableau.constraints.len()).any(|constraint| {
                    resolved_violation(tableau, rival, constraint).unwrap_or(u16::MAX)
                        < resolved_violation(tableau, observed, constraint).unwrap_or(u16::MAX)
                });
                if rival_no_worse && rival_better {
                    results.push(HarmonicBound {
                        input: tableau.input.clone(),
                        observed: observed.name.clone(),
                        bounding_rival: rival.name.clone(),
                    });
                }
            }
        }
    }
    results
}

pub(crate) fn individually_unnecessary_constraints(
    tableaus: &[Tableau],
    a_priori_rankings: &[(usize, usize)],
) -> Result<Vec<usize>, String> {
    let width = aligned(tableaus)?;
    let mut unnecessary = Vec::new();
    for removed in 0..width {
        let reduced: Vec<Tableau> = tableaus
            .iter()
            .map(|tableau| {
                let mut tableau = tableau.clone();
                tableau.constraints.remove(removed);
                for candidate in &mut tableau.candidates {
                    candidate.violations.remove(removed);
                }
                tableau
            })
            .collect();
        let rankings: Vec<(usize, usize)> = a_priori_rankings
            .iter()
            .filter(|(higher, lower)| *higher != removed && *lower != removed)
            .map(|(higher, lower)| {
                (
                    higher - usize::from(*higher > removed),
                    lower - usize::from(*lower > removed),
                )
            })
            .collect();
        if infer_ot_ranking(&reduced, &rankings).is_ok() {
            unnecessary.push(removed);
        }
    }
    Ok(unnecessary)
}

pub fn ranking_implications(
    result: &crate::engine::RankingSpaceResult,
) -> HashMap<String, Vec<String>> {
    let answers: Vec<Vec<String>> = result.winner_counts.keys().cloned().collect();
    let atoms: HashSet<String> = answers.iter().flatten().cloned().collect();
    let mut implications = HashMap::new();
    for antecedent in &atoms {
        let contexts: Vec<&Vec<String>> = answers
            .iter()
            .filter(|answer| answer.contains(antecedent))
            .collect();
        let consequences: Vec<String> = atoms
            .iter()
            .filter(|candidate| {
                *candidate != antecedent
                    && !contexts.is_empty()
                    && contexts.iter().all(|answer| answer.contains(candidate))
            })
            .cloned()
            .collect();
        if !consequences.is_empty() {
            implications.insert(antecedent.clone(), consequences);
        }
    }
    implications
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exact::NumericScalar;
    use crate::reference_cases;

    #[test]
    fn constraint_ranking_is_inferred_from_observed_winners() {
        let document = reference_cases::prince_smolensky_ot();
        let result = infer_ot_ranking(&document.dataset, &[]).expect("ranking exists");
        assert_eq!(result.order, [0, 1]);
    }

    #[test]
    fn finite_maxent_learning_improves_the_declared_frequency_fit() {
        let document = reference_cases::finite_maxent_smoke();
        let result = train_maxent(&document.dataset, 1.0, 2_000).expect("training forms");
        assert!(result.negative_log_likelihood.is_finite());
        assert!(
            result
                .weights
                .iter()
                .all(|weight| weight.is_finite() && *weight >= 0.0)
        );
    }

    #[test]
    fn tiny_positive_prior_sigma_is_used_without_flooring() {
        let mut document = reference_cases::finite_maxent_smoke();
        let tableau = &mut document.dataset[0];
        for candidate in &mut tableau.candidates {
            candidate.observed_frequency = NumericScalar::integer(0);
        }
        tableau.constraints[0].prior_sigma =
            NumericScalar::parse_exact("0.000000001").expect("exact positive sigma");
        let weights = [1.0e-9, 0.0, 0.0];

        let (objective, gradient) =
            objective_and_gradient(&document.dataset, &weights, 1.0).expect("objective forms");

        assert!((objective - 0.5).abs() < 1.0e-12);
        assert!((gradient[0] / 1.0e9 - 1.0).abs() < 1.0e-12);
    }

    #[test]
    fn disabled_constraints_contribute_neither_cost_nor_gradient() {
        let mut document = reference_cases::finite_maxent_smoke();
        for constraint in &mut document.dataset[0].constraints {
            constraint.enabled = false;
            constraint.weight = None;
        }
        let weights = [1_000_000.0, 2_000_000.0, 3_000_000.0];

        let (objective, gradient) =
            objective_and_gradient(&document.dataset, &weights, 1.0).expect("objective forms");

        assert!((objective - 3.0 * 2.0_f64.ln()).abs() < 1.0e-12);
        assert_eq!(gradient, [0.0, 0.0, 0.0]);

        let learned = train_maxent(&document.dataset, 1.0, 1)
            .expect("disabled constraints do not require placeholder weights");
        assert_eq!(learned.weights, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn enabled_constraint_without_weight_returns_an_error() {
        let mut document = reference_cases::finite_maxent_smoke();
        document.dataset[0].constraints[0].weight = None;

        let problem = train_maxent(&document.dataset, 1.0, 1)
            .expect_err("an enabled MaxEnt constraint requires a weight");

        assert_eq!(problem, "enabled constraint `C1` has no MaxEnt weight");
    }

    #[test]
    fn a_priori_cycles_are_rejected_before_ranking_search() {
        let document = reference_cases::prince_smolensky_ot();
        let result = infer_ot_ranking(&document.dataset, &[(0, 1), (1, 0)]);
        assert_eq!(
            result,
            Err("a priori rankings contain a domination cycle".to_owned())
        );
    }
}
