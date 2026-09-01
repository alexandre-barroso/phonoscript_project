//! Cross-interface replay of the finite scholarly validation corpus.
//!
//! An authored PhonoScript program and a checked `.ottab` project are two
//! front ends to the same engine.  These tests nevertheless construct their
//! ledgers independently, compare the evaluator-relevant records, and check a
//! third, source-bounded oracle.  A passing comparison therefore cannot be
//! obtained merely by loading one interface's document through the other.
//!
//! The scope distinctions are intentional:
//! - Pater, Kager, Tessier, Anttila--Cho, and the printed Rimi fragments are
//!   source-bounded reproductions;
//! - the dissertation project is a bounded display transcription;
//! - the Finnish-shaped ranking, finite MaxEnt, and serial pairs are synthetic
//!   engine checks;
//! - Goldwater--Johnson is an exact printed ledger whose unavailable fitted
//!   weights must produce a structured refusal.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use phonoscript::document;
use phonoscript::engine::{ComparisonStatus, TableauEvaluation};
use phonoscript::exact::NumericScalar;
use phonoscript::model::{Candidate, Constraint, ConvalgenDocument, EvaluatorKind, Tableau};
use phonoscript::phonological_engine::{EngineError, EngineStage, PhonologicalEngine};
use phonoscript::phonoscript_runtime::{self, RunResult};
use phonoscript::reference_conformance;

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_project(stem: &str) -> ConvalgenDocument {
    let path = manifest()
        .join("fixtures/reference")
        .join(format!("{stem}.ottab"));
    document::load(&path).unwrap_or_else(|error| panic!("{} did not load: {error}", path.display()))
}

fn run_program(relative: &str) -> RunResult {
    let path = manifest().join("validation/analyses").join(relative);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()));
    let result = phonoscript_runtime::run(&source, &ConvalgenDocument::blank());
    assert_script_succeeded(&path, &result);
    result
}

fn assert_script_succeeded(path: &Path, result: &RunResult) {
    let diagnostics = result
        .diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}:{}:{} {}: {}",
                path.display(),
                diagnostic.primary.start.line,
                diagnostic.primary.start.column,
                diagnostic.code,
                diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        result.succeeded(),
        "{} did not execute successfully:\n{diagnostics}",
        path.display()
    );
}

#[derive(Debug, Clone, PartialEq)]
struct ConstraintSemantics {
    weight: Option<NumericScalar>,
    stratum: usize,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct CandidateSemantics {
    form: String,
    marks_by_constraint: BTreeMap<String, u16>,
    base_mass: NumericScalar,
}

fn constraints(tableau: &Tableau) -> BTreeMap<String, ConstraintSemantics> {
    tableau
        .constraints
        .iter()
        .map(|constraint| {
            (
                constraint.name.clone(),
                ConstraintSemantics {
                    weight: constraint.weight.clone(),
                    stratum: constraint.stratum,
                    enabled: constraint.enabled,
                },
            )
        })
        .collect()
}

fn marks_by_constraint(constraints: &[Constraint], candidate: &Candidate) -> BTreeMap<String, u16> {
    constraints
        .iter()
        .zip(&candidate.violations)
        .map(|(constraint, mark)| (constraint.name.clone(), *mark))
        .collect()
}

fn candidates(tableau: &Tableau) -> BTreeMap<String, CandidateSemantics> {
    tableau
        .candidates
        .iter()
        .map(|candidate| {
            (
                candidate.name.clone(),
                CandidateSemantics {
                    form: candidate.form.clone(),
                    marks_by_constraint: marks_by_constraint(&tableau.constraints, candidate),
                    base_mass: candidate.base_mass.clone(),
                },
            )
        })
        .collect()
}

/// Compare the complete first-order data that can affect evaluation. Stable
/// IDs, notes, observed training counts, and display locators are deliberately
/// excluded because they are not evaluator inputs.
fn assert_same_ledger(label: &str, left: &Tableau, right: &Tableau) {
    assert_eq!(left.input, right.input, "{label}: input");
    assert_eq!(
        left.tie_policy_kind(),
        right.tie_policy_kind(),
        "{label}: ties"
    );
    assert_eq!(
        constraints(left),
        constraints(right),
        "{label}: constraints"
    );
    assert_eq!(candidates(left), candidates(right), "{label}: candidates");
}

fn evaluate(
    engine: &PhonologicalEngine,
    project: &ConvalgenDocument,
    tableau: &Tableau,
) -> TableauEvaluation {
    engine
        .evaluate_in_project(project, tableau)
        .unwrap_or_else(|error| panic!("{} was refused: {error}", tableau.name))
}

fn winner_names(tableau: &Tableau, result: &TableauEvaluation) -> Vec<String> {
    let mut names = result
        .winner_indices
        .iter()
        .map(|index| tableau.candidates[*index].name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn ordered_names(tableau: &Tableau, result: &TableauEvaluation) -> Vec<Vec<String>> {
    result
        .ordered_strata
        .iter()
        .map(|stratum| {
            let mut names = stratum
                .iter()
                .map(|index| tableau.candidates[*index].name.clone())
                .collect::<Vec<_>>();
            names.sort();
            names
        })
        .collect()
}

fn row_by_name<'a>(
    tableau: &'a Tableau,
    result: &'a TableauEvaluation,
    name: &str,
) -> &'a phonoscript::engine::CandidateEvaluation {
    result
        .rows
        .iter()
        .find(|row| tableau.candidates[row.candidate].name == name)
        .unwrap_or_else(|| panic!("missing calculated row `{name}` in {}", tableau.name))
}

fn assert_close(actual: f64, expected: f64, coordinate: &str) {
    assert!(
        (actual - expected).abs() <= 1.0e-12,
        "{coordinate}: expected {expected:.16}, calculated {actual:.16}"
    );
}

fn assert_same_evaluation(
    label: &str,
    left_tableau: &Tableau,
    left: &TableauEvaluation,
    right_tableau: &Tableau,
    right: &TableauEvaluation,
) {
    assert_eq!(
        winner_names(left_tableau, left),
        winner_names(right_tableau, right),
        "{label}: winners"
    );
    assert_eq!(
        ordered_names(left_tableau, left),
        ordered_names(right_tableau, right),
        "{label}: complete order"
    );
    for candidate in &left_tableau.candidates {
        let left_row = row_by_name(left_tableau, left, &candidate.name);
        let right_row = row_by_name(right_tableau, right, &candidate.name);
        assert_eq!(
            left_row.exact_harmony, right_row.exact_harmony,
            "{label}.exact_harmony[{}]",
            candidate.name
        );
        assert_close(
            left_row.harmony,
            right_row.harmony,
            &format!("{label}.harmony[{}]", candidate.name),
        );
        match (left_row.probability, right_row.probability) {
            (Some(left), Some(right)) => assert_close(
                left,
                right,
                &format!("{label}.probability[{}]", candidate.name),
            ),
            (None, None) => {}
            values => panic!(
                "{label}: probability type differs for {}: {values:?}",
                candidate.name
            ),
        }
    }
}

fn tableau_by_name<'a>(document: &'a ConvalgenDocument, name: &str) -> &'a Tableau {
    document
        .dataset
        .iter()
        .find(|tableau| tableau.name == name)
        .unwrap_or_else(|| panic!("missing tableau `{name}`"))
}

#[test]
fn pater_source_exact_panels_agree_across_native_fixture_phonoscript_and_ottab() {
    let engine = PhonologicalEngine::new();
    let authored = run_program("published/pater-harmonic-grammar.phont").document;
    let ottab = load_project("pater-hg");
    let [printed_onset, printed_coda] =
        reference_conformance::pater_positional_faithfulness_panels();

    assert_eq!(printed_onset.source_key, "pater2008gradient");
    assert!(printed_onset.locator.contains("Tableau (13)"));
    assert!(
        printed_onset
            .claim_ceiling
            .contains("Exact finite HG panel")
    );
    assert_same_ledger(
        "Pater /da/ native-to-script",
        &printed_onset.tableau,
        &authored.source,
    );
    assert_same_ledger(
        "Pater /tad/ native-to-script",
        &printed_coda.tableau,
        &authored.target,
    );
    assert_same_ledger(
        "Pater /tad/ ottab-to-script",
        &ottab.source,
        &authored.target,
    );
    assert!(!authored.source.source_locator.is_empty());
    assert!(!authored.target.source_locator.is_empty());
    assert!(!ottab.source.source_locator.is_empty());

    let onset = evaluate(&engine, &authored, &authored.source);
    let coda_script = evaluate(&engine, &authored, &authored.target);
    let coda_ottab = evaluate(&engine, &ottab, &ottab.source);
    assert_eq!(winner_names(&authored.source, &onset), ["faithful"]);
    assert_eq!(winner_names(&authored.target, &coda_script), ["devoiced"]);
    assert_close(
        row_by_name(&authored.source, &onset, "faithful").harmony,
        1.5,
        "Pater /da/ faithful cost",
    );
    assert_close(
        row_by_name(&authored.source, &onset, "devoiced").harmony,
        2.0,
        "Pater /da/ devoiced cost",
    );
    assert_close(
        row_by_name(&authored.target, &coda_script, "faithful").harmony,
        1.5,
        "Pater /tad/ faithful cost",
    );
    assert_close(
        row_by_name(&authored.target, &coda_script, "devoiced").harmony,
        1.0,
        "Pater /tad/ devoiced cost",
    );
    assert_same_evaluation(
        "Pater /tad/ cross-interface",
        &authored.target,
        &coda_script,
        &ottab.source,
        &coda_ottab,
    );
}

#[test]
fn tessier_hg_maxent_ledger_agrees_and_the_engine_corrects_the_printed_probability() {
    let engine = PhonologicalEngine::new();
    let authored = run_program("published/tessier-hg-maxent.phont").document;
    let ottab = load_project("tessier-hg-maxent");
    let printed = reference_conformance::tessier_skul_hg_maxent();

    assert_eq!(printed.source_key, "tessier2017learnability");
    assert!(printed.locator.contains("Tableaux (14)-(15)"));
    assert!(printed.claim_ceiling.contains("erroneous decimal"));
    assert_same_ledger(
        "Tessier native-to-script HG",
        &printed.tableau,
        &authored.source,
    );
    assert_same_ledger(
        "Tessier native-to-script MaxEnt",
        &printed.tableau,
        &authored.target,
    );
    assert_same_ledger(
        "Tessier script-to-ottab HG",
        &authored.source,
        &ottab.source,
    );
    assert_same_ledger(
        "Tessier script-to-ottab MaxEnt",
        &authored.target,
        &ottab.target,
    );
    for tableau in [
        &authored.source,
        &authored.target,
        &ottab.source,
        &ottab.target,
    ] {
        assert!(!tableau.source_locator.is_empty());
    }

    let hg_script = evaluate(&engine, &authored, &authored.source);
    let hg_ottab = evaluate(&engine, &ottab, &ottab.source);
    assert_same_evaluation(
        "Tessier HG cross-interface",
        &authored.source,
        &hg_script,
        &ottab.source,
        &hg_ottab,
    );
    assert_eq!(winner_names(&authored.source, &hg_script), ["delete-s"]);
    for (candidate, expected) in [("faithful", 11.0), ("delete-s", 1.0), ("epenthetic", 2.0)] {
        assert_close(
            row_by_name(&authored.source, &hg_script, candidate).harmony,
            expected,
            &format!("Tessier HG cost[{candidate}]"),
        );
    }

    let maxent_script = evaluate(&engine, &authored, &authored.target);
    let maxent_ottab = evaluate(&engine, &ottab, &ottab.target);
    assert_same_evaluation(
        "Tessier MaxEnt cross-interface",
        &authored.target,
        &maxent_script,
        &ottab.target,
        &maxent_ottab,
    );
    assert_eq!(winner_names(&authored.target, &maxent_script), ["delete-s"]);
    let probabilities = [
        ("faithful", 0.000_033_188_906_581_985_21),
        ("delete-s", 0.731_034_315_595_132_8),
        ("epenthetic", 0.268_932_495_498_285_24),
    ];
    for (candidate, expected) in probabilities {
        assert_close(
            row_by_name(&authored.target, &maxent_script, candidate)
                .probability
                .expect("MaxEnt probability"),
            expected,
            &format!("Tessier MaxEnt probability[{candidate}]"),
        );
    }
}

#[test]
fn source_exact_authored_programs_match_the_independent_native_transcriptions() {
    let engine = PhonologicalEngine::new();

    let kager = run_program("published/kager-coda-voicing.phont").document;
    let [dutch, english] = reference_conformance::kager_dutch_english_final_voicing();
    assert_eq!(dutch.source_key, "kager1999optimality");
    assert!(dutch.locator.contains("Tableau (18)"));
    assert!(english.locator.contains("Tableau (23)"));
    assert!(!kager.source.source_locator.is_empty());
    assert!(!kager.target.source_locator.is_empty());
    assert_same_ledger("Kager Dutch", &dutch.tableau, &kager.source);
    assert_same_ledger("Kager English", &english.tableau, &kager.target);
    assert_eq!(
        winner_names(&kager.source, &evaluate(&engine, &kager, &kager.source)),
        ["devoiced"]
    );
    assert_eq!(
        winner_names(&kager.target, &evaluate(&engine, &kager, &kager.target)),
        ["faithful"]
    );

    let rimi = run_program("published/mccarthy-rimi.phont").document;
    let printed_rimi = reference_conformance::mccarthy_rimi_parallel_and_gen1();
    assert_eq!(printed_rimi.source_key, "mccarthy2000harmonic");
    assert!(printed_rimi.claim_ceiling.contains("printed rows a-b"));
    assert!(!rimi.source.source_locator.is_empty());
    assert!(!rimi.target.source_locator.is_empty());
    assert_eq!(
        constraints(&printed_rimi.parallel),
        constraints(&rimi.source),
        "Rimi parallel constraint register"
    );
    for candidate in &printed_rimi.parallel.candidates {
        let scripted = rimi
            .source
            .candidates
            .iter()
            .find(|item| item.name == candidate.name)
            .expect("scripted Rimi row");
        assert_eq!(
            marks_by_constraint(&printed_rimi.parallel.constraints, candidate),
            marks_by_constraint(&rimi.source.constraints, scripted),
            "Rimi marks for {}",
            candidate.name
        );
    }
    assert_same_ledger(
        "Rimi bounded spreading-first GEN1",
        &printed_rimi.serial_tableau,
        &rimi.target,
    );
    assert_eq!(rimi.target_serial, printed_rimi.serial);
    assert_eq!(
        winner_names(&rimi.source, &evaluate(&engine, &rimi, &rimi.source)),
        ["tone-flop"]
    );
    let serial = engine
        .serial(&rimi.target, &rimi.target_serial, EvaluatorKind::Ot, 1.0)
        .expect("Rimi bounded serial projection is formed");
    assert_eq!(serial.path, ["A: prefix-linked H"]);
    assert_eq!(serial.stopped, "faithful convergence");
}

#[test]
fn anttila_cho_authored_ranking_enumeration_reproduces_all_four_printed_counts() {
    let engine = PhonologicalEngine::new();
    let authored = run_program("published/anttila-cho-linking-r.phont").document;
    let printed = reference_conformance::anttila_cho_linking_r();
    assert_eq!(printed.source_key, "anttilaCho1998variationChange");
    assert!(printed.locator.contains("Table (11)"));
    assert!(
        printed
            .claim_ceiling
            .contains("uniform-tableau interpretation")
    );
    assert_eq!(authored.dataset.len(), 12);

    for competition in printed.competitions {
        let mut counts = [0_u32; 2];
        let prefix = format!("{} · ", competition.label);
        let panels = authored
            .dataset
            .iter()
            .filter(|tableau| tableau.name.starts_with(&prefix))
            .collect::<Vec<_>>();
        assert_eq!(panels.len(), 3, "{} total rankings", competition.label);
        for panel in panels {
            assert!(!panel.source_locator.is_empty(), "{}", panel.name);
            assert_eq!(
                constraints(panel).keys().cloned().collect::<Vec<_>>(),
                ["*CODA", "FAITH", "ONSET"]
            );
            for candidate in &competition.tableau.candidates {
                let scripted = panel
                    .candidates
                    .iter()
                    .find(|item| item.name == candidate.name)
                    .expect("scripted Anttila-Cho row");
                assert_eq!(
                    marks_by_constraint(&competition.tableau.constraints, candidate),
                    marks_by_constraint(&panel.constraints, scripted),
                    "{} marks for {}",
                    competition.label,
                    candidate.name
                );
            }
            let result = evaluate(&engine, &authored, panel);
            assert_eq!(result.winner_indices.len(), 1, "{}", panel.name);
            counts[result.winner_indices[0]] += 1;
        }
        assert_eq!(counts, competition.expected_counts, "{}", competition.label);
    }
}

#[test]
fn synthetic_paired_assets_reproduce_maxent_ranking_and_serial_oracles() {
    let engine = PhonologicalEngine::new();

    let maxent_script = run_program("core/finite-maxent-smoke.phont").document;
    let maxent_ottab = load_project("finite-maxent-smoke");
    assert_same_ledger(
        "synthetic finite MaxEnt",
        &maxent_script.source,
        &maxent_ottab.source,
    );
    let maxent_from_script = evaluate(&engine, &maxent_script, &maxent_script.source);
    let maxent_from_ottab = evaluate(&engine, &maxent_ottab, &maxent_ottab.source);
    assert_same_evaluation(
        "synthetic finite MaxEnt",
        &maxent_script.source,
        &maxent_from_script,
        &maxent_ottab.source,
        &maxent_from_ottab,
    );
    assert_eq!(
        winner_names(&maxent_script.source, &maxent_from_script),
        ["strong", "weak"]
    );
    for name in ["weak", "strong"] {
        let row = row_by_name(&maxent_script.source, &maxent_from_script, name);
        assert_close(row.harmony, 2.0, &format!("MaxEnt harmony[{name}]"));
        assert_close(
            row.probability.expect("MaxEnt probability"),
            0.5,
            &format!("MaxEnt probability[{name}]"),
        );
    }

    let ranking_script = run_program("published/anttila-q-calculus.phont").document;
    let ranking_ottab = load_project("finnish-ranking-space");
    assert_same_ledger(
        "synthetic Finnish-shaped ranking",
        &ranking_script.source,
        &ranking_ottab.source,
    );
    let ranking_from_script = engine
        .q_ranking_space(
            std::slice::from_ref(&ranking_script.source),
            &ranking_script.a_priori_rankings,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("script ranking space");
    let ranking_from_ottab = engine
        .q_ranking_space(
            std::slice::from_ref(&ranking_ottab.source),
            &ranking_ottab.a_priori_rankings,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("ottab ranking space");
    assert_eq!(ranking_from_script.total_rankings.to_string(), "6");
    assert_eq!(
        ranking_from_script.winner_counts,
        ranking_from_ottab.winner_counts
    );
    let script_clone = engine
        .q_clone_audit(
            &ranking_script.source,
            0,
            &ranking_script.a_priori_rankings,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("script clone audit");
    let ottab_clone = engine
        .q_clone_audit(
            &ranking_ottab.source,
            0,
            &ranking_ottab.a_priori_rankings,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("ottab clone audit");
    assert!(script_clone.support_conservative);
    assert!(!script_clone.shares_conservative);
    assert_eq!(script_clone.shifts.len(), 2);
    assert_eq!(
        script_clone.support_conservative,
        ottab_clone.support_conservative
    );
    assert_eq!(
        script_clone.shares_conservative,
        ottab_clone.shares_conservative
    );
    assert_eq!(
        script_clone.before.total_rankings,
        ottab_clone.before.total_rankings
    );
    assert_eq!(
        script_clone.before.winner_counts,
        ottab_clone.before.winner_counts
    );
    assert_eq!(
        script_clone.after.total_rankings,
        ottab_clone.after.total_rankings
    );
    assert_eq!(
        script_clone.after.winner_counts,
        ottab_clone.after.winner_counts
    );
    assert_eq!(script_clone.shifts, ottab_clone.shifts);

    let serial_script = run_program("core/serial-syllabification-smoke.phont").document;
    let serial_ottab = load_project("serial-syllabification-smoke");
    assert_eq!(
        constraints(&serial_script.source),
        constraints(&serial_ottab.source)
    );
    assert_eq!(serial_script.serial, serial_ottab.serial);
    let from_script = engine
        .serial(
            &serial_script.source,
            &serial_script.serial,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("script serial ledger");
    let from_ottab = engine
        .serial(
            &serial_ottab.source,
            &serial_ottab.serial,
            EvaluatorKind::Ot,
            1.0,
        )
        .expect("ottab serial ledger");
    assert_eq!(from_script, from_ottab);
    assert_eq!(from_script.path, ["txznt", "tx(zN)t"]);
    assert_eq!(from_script.operations, ["construct one nucleus"]);
    assert_eq!(from_script.stopped, "faithful convergence");
    assert!(from_script.formed);
}

#[test]
fn dissertation_paired_assets_match_all_bounded_tableaux_and_second_order_oracles() {
    const LOCATOR_ORACLE: [(&str, &str, &str); 39] = [
        (
            "H.1 Neutral HG selection",
            "fig:apph-neutral-selection-tableau",
            "exact",
        ),
        (
            "H.2 Goldrick-Daland x score replay",
            "tab:apph-gd-score-replay",
            "exact-transform",
        ),
        (
            "H.3 Goldrick-Daland w score replay",
            "tab:apph-gd-score-replay",
            "exact-transform",
        ),
        (
            "H.4 Exact tenths-grid objective",
            "tab:apph-mccollum-grid",
            "exact-transform",
        ),
        (
            "H.5 Basic Syllable /CV/",
            "tab:apph-basic-tensor",
            "neutral-ledger",
        ),
        (
            "H.6 Basic Syllable /CVC/",
            "tab:apph-basic-tensor",
            "neutral-ledger",
        ),
        (
            "H.7 Basic Syllable /V/",
            "tab:apph-basic-tensor",
            "neutral-ledger",
        ),
        (
            "H.8 Basic Syllable /VC/",
            "tab:apph-basic-tensor",
            "neutral-ledger",
        ),
        ("H.9 Walker source 1", "tab:apph-walker-replay", "exact"),
        ("H.10 Walker source 2", "tab:apph-walker-replay", "exact"),
        ("H.11 Walker source 3", "tab:apph-walker-replay", "exact"),
        ("H.12 Walker source 4", "tab:apph-walker-replay", "exact"),
        ("H.13 Walker source 5", "tab:apph-walker-replay", "exact"),
        ("H.14 Walker source 6", "tab:apph-walker-replay", "exact"),
        (
            "H.15 Walker interior witness",
            "tab:apph-walker-replay",
            "exact-transform",
        ),
        ("H.16 Walker boundary", "tab:apph-walker-replay", "exact"),
        (
            "H.17 Hidden-candidate MaxEnt fibre",
            "fig:apph-pater-scaling-tableau",
            "snapshot-only",
        ),
        (
            "H.18 One-shot support",
            "fig:apph-cabrera-consumer-tableaux",
            "exact",
        ),
        (
            "H.19 Binary MParse boundary",
            "fig:apph-cabrera-consumer-tableaux",
            "exact",
        ),
        (
            "E.1 Smallest MaxEnt tableau",
            "fig:appe-smallest-maxent",
            "exact-transform",
        ),
        (
            "E.2 Polynomial ledger /u/",
            "fig:appe-polynomial-ledgers",
            "exact-transform",
        ),
        (
            "E.3 Polynomial ledger /u'/",
            "fig:appe-polynomial-ledgers",
            "exact-transform",
        ),
        (
            "1.1 Neutral profile",
            "fig:intro-neutral-profiles",
            "neutral-ledger",
        ),
        (
            "1.2 C1 above C2",
            "fig:intro-ranking-one",
            "exact-transform",
        ),
        (
            "1.3 C2 above C1",
            "fig:intro-ranking-two",
            "exact-transform",
        ),
        (
            "1.4 Candidate-deletion source",
            "fig:intro-deletion-sot",
            "exact-transform",
        ),
        (
            "1.5 Neutral source",
            "fig:intro-sot-opening",
            "exact-transform",
        ),
        ("5.1 Prior-art C1 first", "tab:prior-neutral-ot", "exact"),
        ("5.2 Prior-art C2 first", "tab:prior-neutral-ot", "exact"),
        (
            "5.3 Evaluator-neutral rows",
            "fig:prior-evaluator-neutral",
            "neutral-ledger",
        ),
        (
            "4.1 MaxEnt-opening neutral rows",
            "fig:maxent-opening-profiles",
            "neutral-ledger",
        ),
        (
            "2.1 Calculus neutral source",
            "fig:calc-ex01-sot",
            "exact-transform",
        ),
        (
            "2.4 Four-question source",
            "tab:calc-four-question-matrix",
            "exact",
        ),
        (
            "2.5 Four-question target",
            "tab:calc-four-question-matrix",
            "exact",
        ),
        (
            "5.4 Neutral merger",
            "fig:prior-neutral-merger",
            "snapshot-only",
        ),
        (
            "2.2 Serial source path",
            "fig:calc-stacked-serial-sot",
            "flattened-panel",
        ),
        (
            "2.3 Serial target path",
            "fig:calc-stacked-serial-sot",
            "flattened-panel",
        ),
        (
            "2.6 Merger source fibre",
            "fig:calc-ex09-sot",
            "source-panel-only",
        ),
        (
            "2.7 Refusal source fibre",
            "fig:calc-ex10-sot",
            "source-panel-only",
        ),
    ];
    let engine = PhonologicalEngine::new();
    let scripted = run_program("dissertation/dissertation-complete.phont").document;
    let ottab = load_project("dissertation-complete");
    assert_eq!(scripted.dataset.len(), 39);
    assert_eq!(ottab.dataset.len(), 39);

    for (name, label, fidelity) in LOCATOR_ORACLE {
        let stored = tableau_by_name(&ottab, name);
        let authored = tableau_by_name(&scripted, name);
        assert_eq!(
            stored.source_locator, authored.source_locator,
            "{name}: locator"
        );
        assert!(
            stored.source_locator.contains(label),
            "{name}: missing stable label {label} in {}",
            stored.source_locator
        );
        assert!(
            stored
                .source_locator
                .contains(&format!("fidelity:{fidelity}")),
            "{name}: missing fidelity:{fidelity} in {}",
            stored.source_locator
        );
        assert!(
            !stored.source_locator.contains("display "),
            "{name}: invented display locator survived"
        );
    }

    let mut winner_oracles = 0_usize;
    let mut neutral_records = 0_usize;
    for stored in &ottab.dataset {
        let authored = tableau_by_name(&scripted, &stored.name);
        assert_same_ledger(&stored.name, stored, authored);
        let stored_result = evaluate(&engine, &ottab, stored);
        let authored_result = evaluate(&engine, &scripted, authored);
        assert_same_evaluation(
            &stored.name,
            stored,
            &stored_result,
            authored,
            &authored_result,
        );
        if stored.expected_winners.is_empty() {
            neutral_records += 1;
        } else {
            assert_eq!(
                winner_names(stored, &stored_result),
                stored.expected_winners,
                "{} ({})",
                stored.name,
                stored.source_locator
            );
            winner_oracles += 1;
        }
    }
    assert_eq!(winner_oracles, 32);
    assert_eq!(neutral_records, 7);

    assert_same_ledger("dissertation source", &scripted.source, &ottab.source);
    assert_same_ledger("dissertation target", &scripted.target, &ottab.target);
    let scripted_comparison = engine.compare(&scripted);
    let stored_comparison = engine.compare(&ottab);
    assert_eq!(scripted_comparison.status, ComparisonStatus::Discrepant);
    assert_eq!(scripted_comparison.discrepancies.len(), 2);
    assert_eq!(scripted_comparison.status, stored_comparison.status);
    assert_eq!(
        scripted_comparison.source_answer,
        stored_comparison.source_answer
    );
    assert_eq!(
        scripted_comparison.target_answer,
        stored_comparison.target_answer
    );
    assert_eq!(
        scripted_comparison.discrepancies,
        stored_comparison.discrepancies
    );

    let focused_script = run_program("dissertation/dissertation-second-order.phont").document;
    let focused_ottab = load_project("dissertation-second-order");
    assert_same_ledger(
        "focused dissertation source",
        &focused_script.source,
        &focused_ottab.source,
    );
    assert_same_ledger(
        "focused dissertation target",
        &focused_script.target,
        &focused_ottab.target,
    );
    for (side, scripted_tableau, stored_tableau) in [
        ("source", &focused_script.source, &focused_ottab.source),
        ("target", &focused_script.target, &focused_ottab.target),
    ] {
        assert_eq!(
            scripted_tableau.source_locator, stored_tableau.source_locator,
            "focused dissertation {side}: locator"
        );
        assert!(
            stored_tableau
                .source_locator
                .contains("fig:intro-sot-opening")
        );
        assert!(
            stored_tableau
                .source_locator
                .contains("fidelity:exact-transform")
        );
    }
    let focused_script_result = engine.compare(&focused_script);
    let focused_ottab_result = engine.compare(&focused_ottab);
    assert_eq!(focused_script_result.status, ComparisonStatus::Discrepant);
    assert_eq!(focused_script_result.discrepancies.len(), 2);
    assert_eq!(
        focused_script_result.source_answer,
        focused_ottab_result.source_answer
    );
    assert_eq!(
        focused_script_result.target_answer,
        focused_ottab_result.target_answer
    );
    assert_eq!(
        focused_script_result.discrepancies,
        focused_ottab_result.discrepancies
    );
}

#[test]
fn source_exact_goldwater_ledger_preserves_structured_refusal_across_interfaces() {
    let engine = PhonologicalEngine::new();
    let authored = run_program("published/goldwater-johnson-table-2.phont").document;
    let ottab = load_project("goldwater-johnson-finnish-ledger");
    let printed = reference_conformance::goldwater_johnson_finnish_report();
    assert_eq!(printed.source_key, "goldwater2003learning");
    assert!(printed.ledger_locator.contains("Table 2"));
    assert!(printed.difference_locator.contains("Table 3"));
    assert!(printed.probability_report_locator.contains("Table 4"));
    assert!(printed.claim_ceiling.contains("weight vector is absent"));
    assert_eq!(ottab.dataset.len(), 4);
    assert_eq!(authored.dataset.len(), 4);
    assert_eq!(ottab.dataset, printed.ledger.dataset);

    let emitted = phonoscript_runtime::try_emit(&ottab).expect("emit .ottab as PhonoScript");
    let restored = phonoscript_runtime::run(&emitted, &ConvalgenDocument::blank());
    assert_script_succeeded(Path::new("generated Goldwater-Johnson restore"), &restored);
    assert_eq!(restored.document, ottab);

    for index in 0..4 {
        let native = &printed.ledger.dataset[index];
        let stored = &ottab.dataset[index];
        let scripted = &authored.dataset[index];
        let generated = &restored.document.dataset[index];
        assert_same_ledger(
            &format!("Goldwater-Johnson native-to-authored item {index}"),
            native,
            scripted,
        );
        assert_same_ledger(
            &format!("Goldwater-Johnson ottab-to-authored item {index}"),
            stored,
            scripted,
        );
        assert!(
            scripted
                .constraints
                .iter()
                .all(|constraint| constraint.weight.is_none()),
            "item {index} substituted a weight"
        );
        assert_eq!(scripted.missing_dependencies, native.missing_dependencies);
        assert_eq!(scripted.source_locator, native.source_locator);
        let stored_error = engine
            .evaluate(stored, EvaluatorKind::MaxEnt, 1.0)
            .expect_err("unpublished weights must refuse .ottab evaluation");
        let scripted_error = engine
            .evaluate(scripted, EvaluatorKind::MaxEnt, 1.0)
            .expect_err("unpublished weights must refuse authored PhonoScript evaluation");
        let generated_error = engine
            .evaluate(generated, EvaluatorKind::MaxEnt, 1.0)
            .expect_err("unpublished weights must refuse emitted PhonoScript evaluation");
        assert_refusal(index, &stored_error);
        assert_eq!(stored_error, scripted_error);
        assert_eq!(stored_error, generated_error);
    }

    assert_same_ledger(
        "Goldwater-Johnson authored source copy",
        &printed.ledger.dataset[0],
        &authored.source,
    );
    assert_same_ledger(
        "Goldwater-Johnson authored target copy",
        &printed.ledger.dataset[0],
        &authored.target,
    );
}

fn assert_refusal(index: usize, error: &EngineError) {
    assert_eq!(
        error.code, "PE-ADMIT-MISSING-FITTED-WEIGHTS",
        "item {index}"
    );
    assert_eq!(error.stage, EngineStage::Admission, "item {index}");
    assert_eq!(
        error.coordinate, "constraints.fitted-weights",
        "item {index}"
    );
}
