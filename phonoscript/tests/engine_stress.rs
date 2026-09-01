//! Bounded stress and property checks for the checked phonological engine.
//!
//! These tests intentionally construct their own analyses. They exercise the
//! public API on previously unseen tableaux instead of replaying release
//! fixtures, and keep timing ceilings generous enough for shared CI runners.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use phonoscript::engine::{ComparisonStatus, ContractStage, RankingSpaceBudget};
use phonoscript::exact::NumericScalar;
use phonoscript::model::{
    Candidate, ComparisonMode, Constraint, ConvalgenDocument, EvaluatorKind, QueryKind, SerialMove,
    SerialSettings, Tableau, TiePolicy, next_stable_id,
};
use phonoscript::phonological_engine::{EngineStage, PhonologicalEngine};
use proptest::prelude::*;

const CI_STRESS_BUDGET: Duration = Duration::from_secs(20);

fn constraint(index: usize) -> Constraint {
    Constraint {
        id: format!("constraint-{index}"),
        name: format!("C{index:03}"),
        weight: Some(NumericScalar::integer((index % 11 + 1) as i64)),
        stratum: index,
        enabled: true,
        definition: String::new(),
        prior_mean: NumericScalar::integer(0),
        prior_sigma: NumericScalar::integer(100_000),
    }
}

fn candidate(index: usize, constraint_count: usize, seed: u64) -> Candidate {
    let violations = (0..constraint_count)
        .map(|constraint_index| {
            let mixed = seed
                .wrapping_add((index as u64).wrapping_mul(0x9e37_79b9))
                .wrapping_add((constraint_index as u64).wrapping_mul(0x85eb_ca6b))
                .wrapping_add((index * constraint_index) as u64);
            (mixed % 9) as u16
        })
        .collect();
    Candidate {
        id: format!("candidate-{index}"),
        name: format!("candidate-{index:03}"),
        form: format!("form-{index:03}"),
        violations,
        base_mass: NumericScalar::integer((index % 3 + 1) as i64),
        notes: String::new(),
        observed_frequency: NumericScalar::integer(if index == 0 { 1 } else { 0 }),
        structured: None,
    }
}

fn tableau(constraint_count: usize, candidate_count: usize, seed: u64) -> Tableau {
    Tableau {
        id: format!("tableau-{constraint_count}-{candidate_count}-{seed}"),
        name: format!("{constraint_count} × {candidate_count} stress tableau"),
        input: format!("input-{seed}"),
        constraints: (0..constraint_count).map(constraint).collect(),
        candidates: (0..candidate_count)
            .map(|index| candidate(index, constraint_count, seed))
            .collect(),
        tie_policy: TiePolicy::RetainAll.storage_value().to_owned(),
        notes: String::new(),
        evaluator: None,
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: Vec::new(),
        source_locator: String::new(),
    }
}

fn two_candidate_tableau() -> Tableau {
    let mut tableau = tableau(1, 2, 7);
    tableau.candidates[0].violations = vec![0];
    tableau.candidates[1].violations = vec![1];
    tableau
}

fn project(source: Tableau, target: Tableau, evaluator: EvaluatorKind) -> ConvalgenDocument {
    let mut project = ConvalgenDocument::blank();
    project.evaluator = evaluator;
    project.source = source.clone();
    project.target = target;
    project.dataset = vec![source];
    project.temperature = NumericScalar::integer(1);
    project.second_order.query = QueryKind::WinnerSet;
    project.second_order.answer_sort = "set of candidate identities".to_owned();
    project
}

fn assert_probability_law_is_normalized(
    result: &phonoscript::engine::TableauEvaluation,
    expected_rows: usize,
) {
    assert_eq!(result.rows.len(), expected_rows);
    let probabilities: Vec<f64> = result
        .rows
        .iter()
        .map(|row| row.probability.expect("MaxEnt supplies a probability"))
        .collect();
    assert!(
        probabilities
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
    );
    assert!((probabilities.iter().sum::<f64>() - 1.0).abs() < 1.0e-10);
}

#[test]
fn large_ot_hg_and_maxent_tableaux_are_fast_normalized_and_repeatable() {
    let engine = PhonologicalEngine::new();
    let started = Instant::now();

    for (constraint_count, candidate_count, repetitions) in [(30, 24, 16), (60, 80, 8)] {
        let tableau = tableau(constraint_count, candidate_count, 0x5eed);
        for evaluator in EvaluatorKind::ALL {
            let baseline = engine
                .evaluate(&tableau, evaluator, 1.25)
                .expect("large finite tableau is admitted");
            assert_eq!(baseline.rows.len(), candidate_count);
            assert_eq!(
                baseline.ordered_strata.iter().map(Vec::len).sum::<usize>(),
                candidate_count
            );
            if evaluator == EvaluatorKind::MaxEnt {
                assert_probability_law_is_normalized(&baseline, candidate_count);
            }
            for _ in 0..repetitions {
                let repeated = engine
                    .evaluate(&tableau, evaluator, 1.25)
                    .expect("repeat evaluation remains admitted");
                assert_eq!(repeated, baseline);
            }
        }
    }

    assert!(
        started.elapsed() < CI_STRESS_BUDGET,
        "bounded first-order stress suite exceeded {CI_STRESS_BUDGET:?}"
    );
}

#[test]
fn formation_and_admission_failures_have_stable_coordinates() {
    let engine = PhonologicalEngine::new();
    let valid = two_candidate_tableau();

    for temperature in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let error = engine
            .evaluate(&valid, EvaluatorKind::MaxEnt, temperature)
            .expect_err("invalid temperatures are refused");
        assert_eq!(error.code, "PE-ADMIT-TEMPERATURE");
        assert_eq!(error.stage, EngineStage::Admission);
        assert_eq!(error.coordinate, "evaluator.temperature");
    }

    let mut missing_weight = valid.clone();
    missing_weight.constraints[0].weight = None;
    engine
        .evaluate(&missing_weight, EvaluatorKind::Ot, 1.0)
        .expect("strict OT does not require weights");
    let error = engine
        .evaluate(&missing_weight, EvaluatorKind::HarmonicGrammar, 1.0)
        .expect_err("HG requires a weight");
    assert_eq!(error.code, "PE-ADMIT-MISSING-WEIGHT");
    assert_eq!(error.coordinate, "constraint[0].weight");

    let mut zero_mass = valid.clone();
    zero_mass.candidates[0].base_mass = NumericScalar::integer(0);
    let error = engine
        .evaluate(&zero_mass, EvaluatorKind::MaxEnt, 1.0)
        .expect_err("zero base mass is outside the model domain");
    assert_eq!(error.code, "PE-FORM-BASE-MASS");
    assert_eq!(error.coordinate, "candidate[0]");

    let mut wrong_width = valid;
    wrong_width.candidates[1].violations.push(0);
    let error = engine
        .evaluate(&wrong_width, EvaluatorKind::Ot, 1.0)
        .expect_err("nonrectangular tableaux are refused");
    assert_eq!(error.code, "PE-FORM-MATRIX");
    assert_eq!(error.coordinate, "candidate[1]");
}

#[test]
fn tie_policies_preserve_native_ties_and_apply_only_the_declared_resolution() {
    let engine = PhonologicalEngine::new();
    let mut tableau = two_candidate_tableau();
    tableau.candidates[1].violations = vec![0];

    tableau.set_tie_policy(TiePolicy::RetainAll);
    let retained = engine
        .evaluate(&tableau, EvaluatorKind::Ot, 1.0)
        .expect("tie tableau is formed");
    assert_eq!(retained.native_winner_indices, [0, 1]);
    assert_eq!(retained.winner_indices, [0, 1]);
    assert!(!retained.tie_unresolved);

    tableau.set_tie_policy(TiePolicy::FirstListed);
    let first = engine
        .evaluate(&tableau, EvaluatorKind::Ot, 1.0)
        .expect("declared first-listed policy is admitted");
    assert_eq!(first.native_winner_indices, [0, 1]);
    assert_eq!(first.winner_indices, [0]);

    tableau.set_tie_policy(TiePolicy::RequireUnique);
    let unique = engine
        .evaluate(&tableau, EvaluatorKind::Ot, 1.0)
        .expect("unique-winner policy returns a typed unresolved result");
    assert_eq!(unique.native_winner_indices, [0, 1]);
    assert!(unique.winner_indices.is_empty());
    assert!(unique.tie_unresolved);
}

#[test]
fn stable_ids_are_monotone_unique_and_checked_independently_of_labels() {
    let engine = PhonologicalEngine::new();
    let existing = ["candidate-1", "candidate-8", "published-source-id"];
    assert_eq!(
        next_stable_id("candidate", existing.iter().copied()),
        "candidate-9"
    );

    let mut stable_tableau = two_candidate_tableau();
    stable_tableau.candidates[0].name = "editable label".to_owned();
    stable_tableau.candidates[1].name = "another label".to_owned();
    engine
        .evaluate(&stable_tableau, EvaluatorKind::Ot, 1.0)
        .expect("display-label edits do not change identity");

    stable_tableau.candidates[1].id = stable_tableau.candidates[0].id.clone();
    let error = engine
        .evaluate(&stable_tableau, EvaluatorKind::Ot, 1.0)
        .expect_err("duplicate candidate IDs are refused at the trust boundary");
    assert_eq!(error.code, "PE-FORM-CANDIDATE-ID");
    assert_eq!(error.coordinate, "candidate[1].id");

    let mut tableau = tableau(2, 2, 11);
    tableau.constraints[1].id = tableau.constraints[0].id.clone();
    let error = engine
        .evaluate(&tableau, EvaluatorKind::Ot, 1.0)
        .expect_err("duplicate constraint IDs are refused at the trust boundary");
    assert_eq!(error.code, "PE-FORM-CONSTRAINT-ID");
    assert_eq!(error.coordinate, "constraint[1].id");
}

#[test]
fn serial_engine_distinguishes_convergence_cycles_limits_and_gen1_formation() {
    let engine = PhonologicalEngine::new();
    let tableau = tableau(1, 1, 13);
    let convergent = SerialSettings {
        start: "x".to_owned(),
        moves: vec![
            SerialMove {
                from: "x".to_owned(),
                to: "y".to_owned(),
                operation: "change".to_owned(),
                violations: vec![0],
            },
            SerialMove {
                from: "x".to_owned(),
                to: "x".to_owned(),
                operation: "identity".to_owned(),
                violations: vec![1],
            },
            SerialMove {
                from: "y".to_owned(),
                to: "y".to_owned(),
                operation: "identity".to_owned(),
                violations: vec![0],
            },
        ],
        maximum_steps: 8,
    };
    let result = engine
        .serial(&tableau, &convergent, EvaluatorKind::Ot, 1.0)
        .expect("formed GEN1 ledger");
    assert!(result.formed);
    assert_eq!(result.path, ["x", "y"]);
    assert_eq!(result.stopped, "faithful convergence");

    let cycling = SerialSettings {
        start: "x".to_owned(),
        moves: vec![
            SerialMove {
                from: "x".to_owned(),
                to: "y".to_owned(),
                operation: "forward".to_owned(),
                violations: vec![0],
            },
            SerialMove {
                from: "y".to_owned(),
                to: "x".to_owned(),
                operation: "back".to_owned(),
                violations: vec![0],
            },
        ],
        maximum_steps: 8,
    };
    let result = engine
        .serial(&tableau, &cycling, EvaluatorKind::Ot, 1.0)
        .expect("rectangular cycle ledger is formed before evaluation");
    assert!(!result.formed);
    assert_eq!(result.path, ["x", "y", "x"]);
    assert_eq!(result.stopped, "refused: cycle detected");

    let bounded = SerialSettings {
        start: "x".to_owned(),
        moves: vec![SerialMove {
            from: "x".to_owned(),
            to: "y".to_owned(),
            operation: "change".to_owned(),
            violations: vec![0],
        }],
        maximum_steps: 1,
    };
    let result = engine
        .serial(&tableau, &bounded, EvaluatorKind::Ot, 1.0)
        .expect("bounded ledger is formed");
    assert!(!result.formed);
    assert_eq!(result.stopped, "refused: declared step limit reached");

    let malformed = SerialSettings {
        start: "x".to_owned(),
        moves: vec![SerialMove {
            from: "x".to_owned(),
            to: "x".to_owned(),
            operation: "identity".to_owned(),
            violations: vec![0, 0],
        }],
        maximum_steps: 1,
    };
    let error = engine
        .serial(&tableau, &malformed, EvaluatorKind::Ot, 1.0)
        .expect_err("GEN1 rows must align with the constraint register");
    assert_eq!(error.code, "PE-FORM-SERIAL-MATRIX");
    assert_eq!(error.coordinate, "serial.move[0].violations");
}

#[test]
fn second_order_engine_returns_preservation_discrepancy_and_not_evaluated() {
    let engine = PhonologicalEngine::new();
    let source = two_candidate_tableau();
    let mut analysis = project(source.clone(), source.clone(), EvaluatorKind::Ot);

    let preserved = engine.compare(&analysis);
    assert_eq!(preserved.status, ComparisonStatus::Preserved);
    assert!(preserved.certificate.is_some());
    assert!(preserved.refusal.is_none());

    analysis.target.candidates[0].violations = vec![2];
    analysis.target.candidates[1].violations = vec![0];
    let discrepant = engine.compare(&analysis);
    assert_eq!(discrepant.status, ComparisonStatus::Discrepant);
    assert!(!discrepant.discrepancies.is_empty());
    assert!(discrepant.refusal.is_none());

    analysis.second_order.transport.clear();
    let refused = engine.compare(&analysis);
    assert_eq!(refused.status, ComparisonStatus::NotEvaluated);
    let refusal = refused.refusal.expect("missing transport is indexed");
    assert_eq!(refusal.stage, ContractStage::Formation);
    assert_eq!(refusal.coordinate, "transport");
}

#[test]
fn exact_probability_comparison_never_certifies_an_approximate_weight() {
    let engine = PhonologicalEngine::new();
    let source = two_candidate_tableau();
    let mut target = source.clone();
    target.constraints[0].weight = Some(
        NumericScalar::gui_approximate(1.0)
            .expect("finite approximate parameter has boundary metadata"),
    );
    let mut analysis = project(source, target, EvaluatorKind::MaxEnt);
    analysis.second_order.query = QueryKind::ProbabilityLaw;
    analysis.second_order.answer_sort = "probability law".to_owned();

    let exact = engine.compare(&analysis);
    assert_eq!(exact.status, ComparisonStatus::NotEvaluated);
    let refusal = exact.refusal.expect("exactness boundary is explicit");
    assert_eq!(refusal.code, "QC-APPROXIMATE-WEIGHT");
    assert_eq!(refusal.stage, ContractStage::Certification);
    assert!(exact.certificate.is_none());

    analysis.second_order.comparison_mode = ComparisonMode::Approximate;
    analysis.second_order.tolerance = NumericScalar::parse_exact("0.000000001").unwrap();
    let approximate = engine.compare(&analysis);
    assert_eq!(approximate.status, ComparisonStatus::Preserved);
    let certificate = approximate
        .certificate
        .as_ref()
        .expect("approximate preservation has a scoped certificate");
    assert!(certificate.statement.contains("approximate comparison"));
    assert!(
        certificate
            .evidence
            .iter()
            .any(|item| item.starts_with("absolute tolerance:"))
    );
}

#[test]
fn q_calculus_limit_and_bad_clone_requests_are_structured_refusals() {
    let engine = PhonologicalEngine::new();
    let oversized_candidate_support = tableau(60, 80, 19);
    let error = engine
        .q_ranking_space(
            std::slice::from_ref(&oversized_candidate_support),
            &[],
            EvaluatorKind::Ot,
            1.0,
        )
        .expect_err("Q ranking space has an explicit finite bitset boundary");
    assert_eq!(error.code, "PE-Q-RANKING-SPACE");
    assert_eq!(error.stage, EngineStage::Search);
    assert_eq!(error.coordinate, "q-calculus.ranking-space");
    assert!(error.message.contains("constraint-aligned"));

    let ordinary = two_candidate_tableau();
    let error = engine
        .q_clone_audit(
            &ordinary,
            ordinary.constraints.len(),
            &[],
            EvaluatorKind::Ot,
            1.0,
        )
        .expect_err("out-of-register clone request is refused");
    assert_eq!(error.code, "PE-Q-CLONE-AUDIT");
    assert_eq!(error.stage, EngineStage::Search);
    assert_eq!(error.coordinate, "q-calculus.clone");
}

#[test]
fn q_calculus_is_exact_beyond_u128_and_honors_only_enabled_constraints() {
    let engine = PhonologicalEngine::new();
    let mut neutral = tableau(35, 1, 23);
    for constraint in &mut neutral.constraints {
        constraint.stratum = 0;
    }
    neutral.candidates[0].violations.fill(0);
    let result = engine
        .q_ranking_space(std::slice::from_ref(&neutral), &[], EvaluatorKind::Ot, 1.0)
        .expect("an observationally neutral 35-constraint register has the exact count 35!");
    assert_eq!(
        result.total_rankings.to_string(),
        "10333147966386144929666651337523200000000"
    );
    assert!(result.total_rankings.to_string().len() > u128::MAX.to_string().len());

    neutral.constraints[7].enabled = false;
    let without_disabled = engine
        .q_ranking_space(&[neutral], &[], EvaluatorKind::Ot, 1.0)
        .expect("a disabled constraint is absent from the ranking carrier");
    assert_eq!(
        without_disabled.total_rankings.to_string(),
        "295232799039604140847618609643520000000"
    );
}

#[test]
fn q_calculus_refuses_unregistered_evaluators_ties_relations_and_state_growth() {
    let engine = PhonologicalEngine::new();
    let ordinary = two_candidate_tableau();

    for evaluator in [EvaluatorKind::HarmonicGrammar, EvaluatorKind::MaxEnt] {
        let error = engine
            .q_ranking_space(std::slice::from_ref(&ordinary), &[], evaluator, 1.0)
            .expect_err("Q must not silently apply strict-OT semantics to a weighted evaluator");
        assert_eq!(error.code, "PE-Q-EVALUATOR");
        assert_eq!(error.stage, EngineStage::Admission);
        assert_eq!(error.coordinate, "tableau[0].evaluator");
    }

    let relation_error = engine
        .q_ranking_space(
            std::slice::from_ref(&ordinary),
            &[(0, 0)],
            EvaluatorKind::Ot,
            1.0,
        )
        .expect_err("project a-priori relations have no silently invented Q semantics");
    assert_eq!(relation_error.code, "PE-Q-A-PRIORI-UNSUPPORTED");
    assert_eq!(relation_error.stage, EngineStage::Admission);

    let mut first_listed = ordinary.clone();
    first_listed.set_tie_policy(TiePolicy::FirstListed);
    let first_listed_error = engine
        .q_ranking_space(&[first_listed], &[], EvaluatorKind::Ot, 1.0)
        .expect_err("Q must not silently replace retained winner sets with row-order tie breaking");
    assert_eq!(first_listed_error.code, "PE-Q-TIE-POLICY");
    assert_eq!(first_listed_error.stage, EngineStage::Admission);

    let mut unique = ordinary.clone();
    unique.set_tie_policy(TiePolicy::RequireUnique);
    let unique_error = engine
        .q_ranking_space(&[unique], &[], EvaluatorKind::Ot, 1.0)
        .expect_err("unresolved unique-winner worlds require a declared response type");
    assert_eq!(unique_error.code, "PE-Q-UNIQUE-WINNER");
    assert_eq!(unique_error.stage, EngineStage::Admission);

    let mut branching = tableau(2, 2, 29);
    for constraint in &mut branching.constraints {
        constraint.stratum = 0;
    }
    branching.candidates[0].violations = vec![0, 1];
    branching.candidates[1].violations = vec![1, 0];
    let budget = RankingSpaceBudget::new(1).expect("positive test budget");
    let budget_error = engine
        .q_ranking_space_with_budget(&[branching], &[], EvaluatorKind::Ot, 1.0, budget)
        .expect_err("state growth beyond the declared budget is refused");
    assert_eq!(budget_error.code, "PE-Q-STATE-BUDGET");
    assert_eq!(budget_error.stage, EngineStage::Search);
    assert_eq!(budget_error.coordinate, "q-calculus.ranking-space");
    assert!(budget_error.message.contains("1-state budget"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_bounded_finite_tableaux_are_deterministic_and_maxent_normalizes(
        seed in any::<u64>(),
        constraint_count in 1_usize..16,
        candidate_count in 1_usize..40,
    ) {
        let engine = PhonologicalEngine::new();
        let tableau = tableau(constraint_count, candidate_count, seed);
        let mut seen = HashSet::new();
        for evaluator in EvaluatorKind::ALL {
            let first = engine.evaluate(&tableau, evaluator, 0.75).unwrap();
            let second = engine.evaluate(&tableau, evaluator, 0.75).unwrap();
            prop_assert_eq!(&first, &second);
            prop_assert_eq!(first.rows.len(), candidate_count);
            prop_assert_eq!(
                first.ordered_strata.iter().map(Vec::len).sum::<usize>(),
                candidate_count
            );
            if evaluator == EvaluatorKind::MaxEnt {
                let total: f64 = first
                    .rows
                    .iter()
                    .map(|row| row.probability.unwrap())
                    .sum();
                let probability_domain_is_valid = first.rows.iter().all(|row| {
                    row.probability
                        .is_some_and(|probability| probability.is_finite() && probability >= 0.0)
                });
                prop_assert!(probability_domain_is_valid);
                prop_assert!((total - 1.0).abs() < 1.0e-10);
            }
            seen.insert(format!("{evaluator:?}:{:?}", first.winner_indices));
        }
        prop_assert_eq!(seen.len(), 3);
    }
}
