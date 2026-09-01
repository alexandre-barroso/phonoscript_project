use std::hint::black_box;
use std::time::Instant;

use phonoscript::exact::NumericScalar;
use phonoscript::model::{Candidate, Constraint, EvaluatorKind, Tableau};
use phonoscript::phonological_engine::PhonologicalEngine;
use phonoscript::reference_cases;
use serde::Serialize;

#[derive(Serialize)]
struct BenchmarkReport {
    status: &'static str,
    constraints: usize,
    tableaus: usize,
    candidates_per_tableau: usize,
    total_rankings: String,
    answer_classes: usize,
    dynamic_states: usize,
    completion_states: usize,
    state_budget: usize,
    elapsed_microseconds: u128,
    budget_milliseconds: u128,
    first_order_evaluations: usize,
    first_order_cells: usize,
    first_order_elapsed_microseconds: u128,
    first_order_budget_milliseconds: u128,
    second_order_comparisons: usize,
    second_order_elapsed_microseconds: u128,
    second_order_budget_milliseconds: u128,
}

fn fixture() -> Vec<Tableau> {
    let constraints: Vec<Constraint> = (0..16)
        .map(|index| Constraint {
            id: format!("constraint-{index}"),
            name: format!("C{index:02}"),
            weight: Some(NumericScalar::integer((index % 7 + 1) as i64)),
            stratum: 0,
            enabled: true,
            definition: String::new(),
            prior_mean: NumericScalar::integer(0),
            prior_sigma: NumericScalar::integer(100_000),
        })
        .collect();
    (0..4)
        .map(|tableau_index| Tableau {
            id: format!("tableau-{tableau_index}"),
            name: format!("Benchmark tableau {}", tableau_index + 1),
            input: format!("input-{tableau_index}"),
            constraints: constraints.clone(),
            candidates: (0..6)
                .map(|candidate_index| Candidate {
                    id: format!("candidate-{candidate_index}"),
                    name: format!("candidate-{candidate_index}"),
                    form: format!("output-{candidate_index}"),
                    violations: (0..16)
                        .map(|constraint_index| {
                            ((candidate_index * 7 + constraint_index * 3 + tableau_index * 5) % 5)
                                as u16
                        })
                        .collect(),
                    base_mass: NumericScalar::integer(1),
                    notes: String::new(),
                    observed_frequency: NumericScalar::integer(if candidate_index == 0 {
                        1
                    } else {
                        0
                    }),
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
        })
        .collect()
}

fn main() {
    const BUDGET_MS: u128 = 1000;
    const FIRST_ORDER_EVALUATIONS: usize = 50_000;
    const FIRST_ORDER_BUDGET_MS: u128 = 1000;
    const SECOND_ORDER_COMPARISONS: usize = 20_000;
    const SECOND_ORDER_BUDGET_MS: u128 = 1000;
    let tableaus = fixture();
    let engine = PhonologicalEngine::new();
    let started = Instant::now();
    let result = engine
        .q_ranking_space(&tableaus, &[], EvaluatorKind::Ot, 1.0)
        .expect("benchmark fixture is formed");
    let elapsed = started.elapsed();
    let first_order_started = Instant::now();
    for _ in 0..FIRST_ORDER_EVALUATIONS {
        black_box(
            engine
                .evaluate(black_box(&tableaus[0]), EvaluatorKind::MaxEnt, 1.0)
                .expect("benchmark tableau remains formed"),
        );
    }
    let first_order_elapsed = first_order_started.elapsed();

    let comparison = reference_cases::dissertation_second_order();
    let second_order_started = Instant::now();
    for _ in 0..SECOND_ORDER_COMPARISONS {
        black_box(engine.compare(black_box(&comparison)));
    }
    let second_order_elapsed = second_order_started.elapsed();

    let status = if elapsed.as_millis() <= BUDGET_MS
        && first_order_elapsed.as_millis() <= FIRST_ORDER_BUDGET_MS
        && second_order_elapsed.as_millis() <= SECOND_ORDER_BUDGET_MS
    {
        "PASS"
    } else {
        "FAIL"
    };
    let report = BenchmarkReport {
        status,
        constraints: 16,
        tableaus: 4,
        candidates_per_tableau: 6,
        total_rankings: result.total_rankings.to_string(),
        answer_classes: result.winner_counts.len(),
        dynamic_states: result.dynamic_states,
        completion_states: result.completion_states,
        state_budget: result.state_budget,
        elapsed_microseconds: elapsed.as_micros(),
        budget_milliseconds: BUDGET_MS,
        first_order_evaluations: FIRST_ORDER_EVALUATIONS,
        first_order_cells: FIRST_ORDER_EVALUATIONS
            * tableaus[0].candidates.len()
            * tableaus[0].constraints.len(),
        first_order_elapsed_microseconds: first_order_elapsed.as_micros(),
        first_order_budget_milliseconds: FIRST_ORDER_BUDGET_MS,
        second_order_comparisons: SECOND_ORDER_COMPARISONS,
        second_order_elapsed_microseconds: second_order_elapsed.as_micros(),
        second_order_budget_milliseconds: SECOND_ORDER_BUDGET_MS,
    };
    println!(
        "{}",
        serde_json::to_string(&report).expect("serializable report")
    );
    if status == "FAIL" {
        std::process::exit(1);
    }
}
