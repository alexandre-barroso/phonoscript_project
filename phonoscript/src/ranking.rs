//! Exact finite partial-ranking and Constraint Demotion machinery.
//!
//! This module keeps learning data distinct from ordinary tableaux. A row is
//! a winner--loser comparison whose two cells contain cancelled mark types,
//! following Kager's presentation of Tesar and Smolensky's algorithm.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use crate::model::Tableau;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkDatum {
    pub winner_candidate: usize,
    pub loser_candidate: usize,
    /// Constraints on which the losing candidate is worse.
    pub loser_marks: Vec<usize>,
    /// Constraints on which the attested winner is worse.
    pub winner_marks: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardedMarkDatum {
    pub winner_candidate: usize,
    pub loser_candidate: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkData {
    pub constraint_names: Vec<String>,
    pub rows: Vec<MarkDatum>,
    pub discarded: Vec<DiscardedMarkDatum>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemotionStep {
    pub pass: usize,
    pub datum: usize,
    pub loser_constraint: usize,
    pub demoted_constraints: Vec<usize>,
    pub before: Vec<usize>,
    pub after: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ConstraintDemotionResult {
    Learned {
        strata: Vec<Vec<usize>>,
        constraint_strata: Vec<usize>,
        trace: Vec<DemotionStep>,
        unresolved_pairs: Vec<(usize, usize)>,
    },
    Inconsistent {
        code: String,
        message: String,
        trace: Vec<DemotionStep>,
        conflicting_data: Vec<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialRanking {
    pub constraint_names: Vec<String>,
    /// Ordered pairs `(higher, lower)`.
    pub dominance: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum LinearExtensions {
    Complete {
        orders: Vec<Vec<usize>>,
    },
    Truncated {
        orders: Vec<Vec<usize>>,
        limit: usize,
        message: String,
    },
    Refused {
        code: String,
        message: String,
    },
}

/// Construct cancelled mark data for one attested winner against every other
/// candidate. Multiplicity is used during cancellation, then duplicate mark
/// types are collapsed exactly as in Kager's Mark Cancellation step (16c).
pub fn mark_data(tableau: &Tableau, winner: usize) -> Result<MarkData, String> {
    let Some(winner_row) = tableau.candidates.get(winner) else {
        return Err("winner index is outside the candidate set".to_owned());
    };
    if winner_row.violations.len() != tableau.constraints.len()
        || tableau
            .candidates
            .iter()
            .any(|candidate| candidate.violations.len() != tableau.constraints.len())
    {
        return Err("mark-data construction requires a rectangular violation matrix".to_owned());
    }

    let mut rows = Vec::new();
    let mut discarded = Vec::new();
    for (loser, loser_row) in tableau.candidates.iter().enumerate() {
        if loser == winner {
            continue;
        }
        let mut loser_marks = Vec::new();
        let mut winner_marks = Vec::new();
        for (constraint, (loser_value, winner_value)) in loser_row
            .violations
            .iter()
            .zip(&winner_row.violations)
            .enumerate()
        {
            if loser_value > winner_value {
                loser_marks.push(constraint);
            } else if winner_value > loser_value {
                winner_marks.push(constraint);
            }
        }
        if winner_marks.is_empty() {
            discarded.push(DiscardedMarkDatum {
                winner_candidate: winner,
                loser_candidate: loser,
                reason: if loser_marks.is_empty() {
                    "identical violation profiles provide no ranking information"
                } else {
                    "the loser is harmonically bounded; no winner-mark remains after cancellation"
                }
                .to_owned(),
            });
        } else if loser_marks.is_empty() {
            // The declared winner is worse on at least one constraint and
            // better on none. Preserve the contradiction for structured
            // refusal rather than dropping it.
            rows.push(MarkDatum {
                winner_candidate: winner,
                loser_candidate: loser,
                loser_marks,
                winner_marks,
            });
        } else {
            rows.push(MarkDatum {
                winner_candidate: winner,
                loser_candidate: loser,
                loser_marks,
                winner_marks,
            });
        }
    }
    Ok(MarkData {
        constraint_names: tableau
            .constraints
            .iter()
            .map(|constraint| constraint.name.clone())
            .collect(),
        rows,
        discarded,
    })
}

/// Run recursive Constraint Demotion. All constraints begin in one highest
/// stratum. For every datum, the highest currently ranked loser-mark licenses
/// the minimal demotion of each conflicting winner-mark to the immediately
/// following stratum. The process repeats to a fixed point.
pub fn constraint_demotion(data: &MarkData) -> ConstraintDemotionResult {
    let count = data.constraint_names.len();
    if let Some((datum, _)) = data
        .rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.loser_marks.is_empty() && !row.winner_marks.is_empty())
    {
        return ConstraintDemotionResult::Inconsistent {
            code: "CD-NO-LOSER-MARK".to_owned(),
            message: "an attested winner is harmonically bounded by a declared loser".to_owned(),
            trace: Vec::new(),
            conflicting_data: vec![datum],
        };
    }
    if data
        .rows
        .iter()
        .flat_map(|row| {
            row.loser_marks
                .iter()
                .chain(row.winner_marks.iter())
                .copied()
        })
        .any(|constraint| constraint >= count)
    {
        return ConstraintDemotionResult::Inconsistent {
            code: "CD-CONSTRAINT-INDEX".to_owned(),
            message: "mark data refer to a constraint outside the register".to_owned(),
            trace: Vec::new(),
            conflicting_data: Vec::new(),
        };
    }

    let mut ranks = vec![0_usize; count];
    let mut trace = Vec::new();
    let mut seen = HashSet::new();
    let maximum_steps = count
        .saturating_mul(data.rows.len().max(1))
        .saturating_mul(count.max(1))
        .max(1);
    let mut pass = 0;
    loop {
        if !seen.insert(ranks.clone()) {
            return ConstraintDemotionResult::Inconsistent {
                code: "CD-EMPTY-STRATUM-LOOP".to_owned(),
                message: "constraint demotion revisited a hierarchy; input or candidate structure must be reconsidered".to_owned(),
                trace,
                conflicting_data: unsatisfied_rows(data, &ranks),
            };
        }
        let mut changed = false;
        for (datum_index, datum) in data.rows.iter().enumerate() {
            let loser_constraint = *datum
                .loser_marks
                .iter()
                .min_by_key(|constraint| (ranks[**constraint], **constraint))
                .expect("empty loser-mark rows were refused above");
            let loser_rank = ranks[loser_constraint];
            let mut demoted = datum
                .winner_marks
                .iter()
                .copied()
                .filter(|constraint| ranks[*constraint] <= loser_rank)
                .collect::<Vec<_>>();
            demoted.sort_unstable();
            demoted.dedup();
            if demoted.is_empty() {
                continue;
            }
            let before = ranks.clone();
            for constraint in &demoted {
                ranks[*constraint] = loser_rank + 1;
            }
            normalize_ranks(&mut ranks);
            trace.push(DemotionStep {
                pass,
                datum: datum_index,
                loser_constraint,
                demoted_constraints: demoted,
                before,
                after: ranks.clone(),
            });
            changed = true;
            if trace.len() > maximum_steps {
                return ConstraintDemotionResult::Inconsistent {
                    code: "CD-STEP-BOUND".to_owned(),
                    message: "constraint demotion exceeded its finite monotone-state bound"
                        .to_owned(),
                    trace,
                    conflicting_data: unsatisfied_rows(data, &ranks),
                };
            }
        }
        if !changed {
            break;
        }
        pass += 1;
    }

    let conflicts = unsatisfied_rows(data, &ranks);
    if !conflicts.is_empty() {
        return ConstraintDemotionResult::Inconsistent {
            code: "CD-INCONSISTENT-DATA".to_owned(),
            message: "no learned stratum makes every attested winner beat its declared loser"
                .to_owned(),
            trace,
            conflicting_data: conflicts,
        };
    }
    let strata = strata_from_ranks(&ranks);
    let unresolved_pairs = (0..count)
        .flat_map(|left| ((left + 1)..count).map(move |right| (left, right)))
        .filter(|(left, right)| ranks[*left] == ranks[*right])
        .collect();
    ConstraintDemotionResult::Learned {
        strata,
        constraint_strata: ranks,
        trace,
        unresolved_pairs,
    }
}

fn normalize_ranks(ranks: &mut [usize]) {
    let levels = ranks.iter().copied().collect::<BTreeSet<_>>();
    for rank in ranks {
        *rank = levels
            .iter()
            .position(|level| level == rank)
            .expect("rank came from the collected level set");
    }
}

fn strata_from_ranks(ranks: &[usize]) -> Vec<Vec<usize>> {
    let mut strata = vec![Vec::new(); ranks.iter().copied().max().unwrap_or(0) + 1];
    for (constraint, rank) in ranks.iter().copied().enumerate() {
        strata[rank].push(constraint);
    }
    strata
}

fn unsatisfied_rows(data: &MarkData, ranks: &[usize]) -> Vec<usize> {
    data.rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let loser = row.loser_marks.iter().map(|item| ranks[*item]).min();
            let winner = row.winner_marks.iter().map(|item| ranks[*item]).min();
            (loser.is_none() || winner.is_none() || loser >= winner).then_some(index)
        })
        .collect()
}

impl PartialRanking {
    pub fn validate(&self) -> Result<(), String> {
        let count = self.constraint_names.len();
        if self
            .dominance
            .iter()
            .any(|(higher, lower)| higher >= &count || lower >= &count || higher == lower)
        {
            return Err("partial ranking contains an invalid dominance edge".to_owned());
        }
        let extensions = enumerate_linear_extensions(count, &self.dominance, 1);
        match extensions {
            LinearExtensions::Refused { message, .. } => Err(message),
            LinearExtensions::Complete { orders } | LinearExtensions::Truncated { orders, .. }
                if orders.is_empty() =>
            {
                Err("partial ranking contains a cycle".to_owned())
            }
            _ => Ok(()),
        }
    }

    pub fn linear_extensions(&self, limit: usize) -> LinearExtensions {
        enumerate_linear_extensions(self.constraint_names.len(), &self.dominance, limit)
    }
}

pub fn enumerate_linear_extensions(
    constraint_count: usize,
    dominance: &[(usize, usize)],
    limit: usize,
) -> LinearExtensions {
    if limit == 0 {
        return LinearExtensions::Refused {
            code: "PR-LIMIT".to_owned(),
            message: "linear-extension limit must be positive".to_owned(),
        };
    }
    if dominance
        .iter()
        .any(|(higher, lower)| *higher >= constraint_count || *lower >= constraint_count)
    {
        return LinearExtensions::Refused {
            code: "PR-EDGE".to_owned(),
            message: "dominance edge is outside the constraint register".to_owned(),
        };
    }
    let edges = dominance.iter().copied().collect::<BTreeSet<_>>();
    let mut successors = vec![Vec::new(); constraint_count];
    let mut indegree = vec![0_usize; constraint_count];
    for (higher, lower) in edges {
        if higher == lower {
            return LinearExtensions::Refused {
                code: "PR-CYCLE".to_owned(),
                message: "a constraint cannot dominate itself".to_owned(),
            };
        }
        successors[higher].push(lower);
        indegree[lower] += 1;
    }
    for items in &mut successors {
        items.sort_unstable();
    }
    let mut orders = Vec::new();
    let mut prefix = Vec::with_capacity(constraint_count);
    let mut used = vec![false; constraint_count];
    let truncated = extend_orders(
        &successors,
        &mut indegree,
        &mut used,
        &mut prefix,
        &mut orders,
        limit,
    );
    if orders.is_empty() && constraint_count > 0 {
        return LinearExtensions::Refused {
            code: "PR-CYCLE".to_owned(),
            message: "partial ranking contains a dominance cycle".to_owned(),
        };
    }
    if truncated {
        LinearExtensions::Truncated {
            orders,
            limit,
            message: "more linear extensions exist than the declared materialization limit"
                .to_owned(),
        }
    } else {
        LinearExtensions::Complete { orders }
    }
}

fn extend_orders(
    successors: &[Vec<usize>],
    indegree: &mut [usize],
    used: &mut [bool],
    prefix: &mut Vec<usize>,
    orders: &mut Vec<Vec<usize>>,
    limit: usize,
) -> bool {
    if prefix.len() == used.len() {
        if orders.len() == limit {
            return true;
        }
        orders.push(prefix.clone());
        return false;
    }
    let available = (0..used.len())
        .filter(|index| !used[*index] && indegree[*index] == 0)
        .collect::<Vec<_>>();
    if available.is_empty() {
        return false;
    }
    for node in available {
        if orders.len() == limit {
            return true;
        }
        used[node] = true;
        prefix.push(node);
        for successor in &successors[node] {
            indegree[*successor] -= 1;
        }
        let truncated = extend_orders(successors, indegree, used, prefix, orders, limit);
        for successor in &successors[node] {
            indegree[*successor] += 1;
        }
        prefix.pop();
        used[node] = false;
        if truncated {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference_cases;

    #[test]
    fn mark_cancellation_preserves_direction_and_discards_bounded_rows() {
        let project = reference_cases::prince_smolensky_ot();
        let data = mark_data(&project.source, 0).expect("formed mark data");
        assert!(data.rows.is_empty());
        assert_eq!(data.discarded.len(), 1);
        assert!(data.discarded[0].reason.contains("harmonically bounded"));
    }

    #[test]
    fn recursive_demotion_learns_a_partial_hierarchy() {
        let data = MarkData {
            constraint_names: vec!["A".into(), "B".into(), "C".into()],
            rows: vec![
                MarkDatum {
                    winner_candidate: 0,
                    loser_candidate: 1,
                    loser_marks: vec![0],
                    winner_marks: vec![1],
                },
                MarkDatum {
                    winner_candidate: 0,
                    loser_candidate: 2,
                    loser_marks: vec![1],
                    winner_marks: vec![2],
                },
            ],
            discarded: Vec::new(),
        };
        let ConstraintDemotionResult::Learned {
            constraint_strata,
            unresolved_pairs,
            ..
        } = constraint_demotion(&data)
        else {
            panic!("consistent data should be learned");
        };
        assert_eq!(constraint_strata, [0, 1, 2]);
        assert!(unresolved_pairs.is_empty());
    }

    #[test]
    fn harmonically_impossible_winner_is_structurally_refused() {
        let data = MarkData {
            constraint_names: vec!["A".into()],
            rows: vec![MarkDatum {
                winner_candidate: 0,
                loser_candidate: 1,
                loser_marks: Vec::new(),
                winner_marks: vec![0],
            }],
            discarded: Vec::new(),
        };
        assert!(matches!(
            constraint_demotion(&data),
            ConstraintDemotionResult::Inconsistent { code, .. } if code == "CD-NO-LOSER-MARK"
        ));
    }

    #[test]
    fn linear_extensions_are_exact_and_deterministic() {
        let result = enumerate_linear_extensions(3, &[(0, 2)], 10);
        let LinearExtensions::Complete { orders } = result else {
            panic!("small acyclic order should complete");
        };
        assert_eq!(orders, [vec![0, 1, 2], vec![0, 2, 1], vec![1, 0, 2]]);
    }

    #[test]
    fn extension_materialization_reports_truncation_instead_of_completeness() {
        let result = enumerate_linear_extensions(4, &[], 3);
        assert!(matches!(
            result,
            LinearExtensions::Truncated { orders, limit: 3, .. } if orders.len() == 3
        ));
    }
}
