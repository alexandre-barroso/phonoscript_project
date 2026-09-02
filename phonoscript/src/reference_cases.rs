//! Fixed scholarly validation cases. They are test data, not an in-app gallery.

use crate::exact::NumericScalar;
use crate::model::{
    Candidate, Constraint, ConvalgenDocument, DependencyScope, DependencyStage, EvaluatorKind,
    MissingDependency, QueryKind, SecondOrderLayout, SerialMove, Tableau,
};

fn scalar(value: f64) -> NumericScalar {
    NumericScalar::parse_exact(&value.to_string())
        .expect("reference constants have finite decimal spellings")
}

fn constraint(name: &str, weight: f64, stratum: usize) -> Constraint {
    Constraint {
        id: format!("constraint:{name}"),
        name: name.to_owned(),
        weight: Some(scalar(weight)),
        stratum,
        enabled: true,
        definition: String::new(),
        prior_mean: NumericScalar::integer(0),
        prior_sigma: NumericScalar::integer(100_000),
    }
}

fn candidate(name: &str, form: &str, violations: &[u16]) -> Candidate {
    Candidate {
        id: format!("candidate:{name}"),
        name: name.to_owned(),
        form: form.to_owned(),
        violations: violations.to_vec(),
        base_mass: NumericScalar::integer(1),
        notes: String::new(),
        observed_frequency: NumericScalar::integer(0),
        structured: None,
    }
}

fn tableau(input: &str, constraints: Vec<Constraint>, candidates: Vec<Candidate>) -> Tableau {
    Tableau {
        id: format!("tableau:{input}"),
        name: input.to_owned(),
        input: input.to_owned(),
        constraints,
        candidates,
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: None,
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: Vec::new(),
        source_locator: String::new(),
    }
}

/// Prince & Smolensky's two-constraint partial Berber comparison (tableau 15).
pub fn prince_smolensky_ot() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Berber partial candidate comparison".to_owned();
    document.evaluator = EvaluatorKind::Ot;
    document.source = tableau(
        "/ḥaultn/",
        vec![constraint("ONS", 1.0, 0), constraint("HNUC", 1.0, 1)],
        vec![
            candidate("onsetted", "~.wL.~", &[0, 1]),
            candidate("onsetless", "~.ul.~", &[1, 1]),
        ],
    );
    document.source.candidates[0].observed_frequency = NumericScalar::integer(1);
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document
}

/// Pater's HG coda-devoicing tableau (2008, tableau 13, /tad/ panel).
pub fn pater_hg() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Coda devoicing with positional faithfulness".to_owned();
    document.evaluator = EvaluatorKind::HarmonicGrammar;
    document.source = tableau(
        "/tad/",
        vec![
            constraint("*VOICE", 1.5, 0),
            constraint("IDENT-VOICE-ONSET", 1.0, 1),
            constraint("IDENT-VOICE", 1.0, 2),
        ],
        vec![
            candidate("faithful", "[tad]", &[1, 0, 0]),
            candidate("devoiced", "[tat]", &[0, 0, 1]),
        ],
    );
    document.source.candidates[1].observed_frequency = NumericScalar::integer(1);
    document.source.source_locator =
        "Pater 2008, physical PDF p. 8, Tableau (13), /tad/ panel".to_owned();
    document.source.expected_winners = vec!["devoiced".to_owned()];
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document
}

/// A compact numerical smoke test for the finite conditional MaxEnt engine.
/// This is deliberately not attributed to a published tableau.
pub fn finite_maxent_smoke() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Finite MaxEnt normalizer smoke test".to_owned();
    document.evaluator = EvaluatorKind::MaxEnt;
    document.temperature = NumericScalar::integer(1);
    document.source = tableau(
        "/naapuri/",
        vec![
            constraint("C1", 2.0, 0),
            constraint("C2", 1.0, 1),
            constraint("C3", 0.5, 2),
        ],
        vec![
            candidate("weak", "naapurien", &[0, 1, 2]),
            candidate("strong", "naapureiden", &[1, 0, 0]),
        ],
    );
    document.source.candidates[0].observed_frequency = NumericScalar::integer(2);
    document.source.candidates[1].observed_frequency = NumericScalar::integer(1);
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document
}

/// Tessier's `/skul/` HG and MaxEnt panels (2017, physical PDF p. 16,
/// Tableaux 14-15). The document preserves the printed finite support and
/// weights while the engine calculates the corrected normalized probabilities.
pub fn tessier_hg_maxent() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Tessier /skul/ HG-MaxEnt comparison".to_owned();
    document.description = "Independent transcription of Tessier (2017), physical PDF page 16, Tableaux (14)-(15). The source's exp(-11) decimal is incorrect and its MaxEnt values are unnormalized; PhonoScript GUI calculates the corrected finite conditional law from the printed ledger.".to_owned();
    document.evaluator = EvaluatorKind::HarmonicGrammar;
    document.temperature = NumericScalar::integer(1);
    let locator = "Tessier 2017, physical PDF p. 16, Tableaux (14)-(15), /skul/ panels";
    let shared = || {
        let mut item = tableau(
            "/skul/",
            vec![
                constraint("*CC-ONSET", 6.0, 0),
                constraint("*s[stop]-ONSET", 5.0, 1),
                constraint("DEP", 2.0, 2),
                constraint("MAX", 1.0, 3),
            ],
            vec![
                candidate("faithful", "[skul]", &[1, 1, 0, 0]),
                candidate("delete-s", "[kul]", &[0, 0, 0, 1]),
                candidate("epenthetic", "[is.kul]", &[0, 0, 1, 0]),
            ],
        );
        item.source_locator = locator.to_owned();
        item.expected_winners = vec!["delete-s".to_owned()];
        item
    };
    document.source = shared();
    document.source.id = "tableau:tessier-hg".to_owned();
    document.source.name = "Tessier Tableau (14), HG".to_owned();
    document.source.evaluator = Some(EvaluatorKind::HarmonicGrammar);
    document.target = shared();
    document.target.id = "tableau:tessier-maxent".to_owned();
    document.target.name = "Tessier Tableau (15), MaxEnt".to_owned();
    document.target.evaluator = Some(EvaluatorKind::MaxEnt);
    document.dataset = vec![document.source.clone(), document.target.clone()];
    document
}

/// Source-faithful transcription of Goldwater & Johnson (2003), Table 2.
/// The paper does not publish the learned Finnish weight vector, so this
/// fixture certifies the 11-column candidate ledger and does not pretend to
/// reproduce the fitted percentages in its Table 4.
type FinnishLedgerCase<'a> = (&'a str, &'a str, &'a str, &'a [u16], &'a str, &'a [u16]);

pub fn goldwater_johnson_finnish_ledger() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Goldwater-Johnson Finnish candidate ledger".to_owned();
    document.description = "Exact transcription of the four word classes displayed in Goldwater and Johnson (2003), Table 2. The Finnish fitted weights are not printed in the paper, so the persisted dependency record makes this a nonevaluable mark ledger rather than a probability-replication claim.".to_owned();
    document.evaluator = EvaluatorKind::MaxEnt;
    let register = [
        "STRESS-TO-WEIGHT",
        "WEIGHT-TO-STRESS",
        "*Í",
        "*Ó",
        "*Á",
        "*Ĭ",
        "*Ŏ",
        "*Ă",
        "*H.H",
        "*L.L",
        "*LAPSE",
    ];
    let cases: [FinnishLedgerCase<'_>; 4] = [
        (
            "kala",
            "weak",
            "ká.lo.jen",
            &[1, 1, 0, 0, 0, 0, 0, 1, 0, 1, 1],
            "strong",
            &[1, 2, 0, 0, 0, 0, 0, 1, 1, 0, 1],
        ),
        (
            "naapuri",
            "weak",
            "náa.pu.ri.en",
            &[0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 2],
            "strong",
            &[0, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0],
        ),
        (
            "ministeri",
            "weak",
            "mı́.nis.te.ri.en",
            &[1, 2, 0, 0, 0, 1, 0, 0, 0, 1, 3],
            "strong",
            &[1, 2, 1, 0, 0, 0, 0, 0, 1, 0, 1],
        ),
        (
            "maailma",
            "weak",
            "máa.il.mo.jen",
            &[0, 2, 0, 0, 0, 0, 0, 1, 1, 0, 2],
            "strong",
            &[0, 2, 0, 0, 1, 0, 0, 0, 3, 0, 0],
        ),
    ];
    document.dataset = cases
        .into_iter()
        .map(|(input, weak_name, weak_form, weak_marks, strong_name, strong_marks)| {
            let mut item = tableau(
                input,
                register
                    .iter()
                    .enumerate()
                    .map(|(index, name)| constraint(name, 0.0, index))
                    .collect(),
                vec![
                    candidate(weak_name, weak_form, weak_marks),
                    candidate(
                        strong_name,
                        match input {
                            "kala" => "ká.loi.den",
                            "naapuri" => "náa.pu.rèi.den",
                            "ministeri" => "mı́.nis.te.rèi.den",
                            _ => "máa.il.mòi.den",
                        },
                        strong_marks,
                    ),
                ],
            );
            item.name = format!("Goldwater-Johnson Table 2: {input}");
            item.id = format!("goldwater-johnson-table-2:{input}");
            item.evaluator = Some(EvaluatorKind::MaxEnt);
            for constraint in &mut item.constraints {
                constraint.weight = None;
            }
            item.missing_dependencies.push(MissingDependency {
                code: "PE-ADMIT-MISSING-FITTED-WEIGHTS".to_owned(),
                stage: DependencyStage::Admission,
                coordinate: "constraints.fitted-weights".to_owned(),
                scope: DependencyScope::Evaluator {
                    evaluator: EvaluatorKind::MaxEnt,
                },
                message: "Goldwater and Johnson (2003) do not publish the Finnish fitted weight vector required to evaluate this MaxEnt ledger".to_owned(),
                remedy: "Supply a source-verified fitted weight vector; do not infer weights from the displayed percentages.".to_owned(),
            });
            item.source_locator =
                "Goldwater & Johnson 2003, Table 2, PDF physical page 6".to_owned();
            item.notes = "Published candidate and violation transcription; Finnish fitted weights are not published in the article.".to_owned();
            item
        })
        .collect();
    document.source = document.dataset[0].clone();
    document.target = document.source.clone();
    document
}

/// A compact finite GEN1 architecture test. The forms are not a transcription
/// of a McCarthy source tableau and are therefore not labelled as one.
pub fn serial_syllabification_smoke() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Finite GEN1 serial smoke test".to_owned();
    document.evaluator = EvaluatorKind::Ot;
    document.source = tableau(
        "/txznt/",
        vec![constraint("ONS", 1.0, 0), constraint("HNUC", 1.0, 1)],
        vec![candidate("first local winner", "tx(zN)t", &[0, 0])],
    );
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document.serial.start = "txznt".to_owned();
    document.serial.moves = vec![
        SerialMove {
            from: "txznt".to_owned(),
            to: "txznt".to_owned(),
            operation: "identity".to_owned(),
            violations: vec![1, 2],
        },
        SerialMove {
            from: "txznt".to_owned(),
            to: "tx(zN)t".to_owned(),
            operation: "construct one nucleus".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "tx(zN)t".to_owned(),
            to: "tx(zN)t".to_owned(),
            operation: "identity".to_owned(),
            violations: vec![0, 0],
        },
    ];
    document.serial.maximum_steps = 8;
    document
}

/// Label-preserving reparameterization of the dissertation's opening
/// Second-Order Tableau: the winner survives while the two losers reverse.
pub fn dissertation_second_order() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Same winner, reversed loser order".to_owned();
    document.evaluator = EvaluatorKind::Ot;
    let constraints = vec![
        constraint("IDENT-IO(voice)", 1.0, 0),
        constraint("MAX-IO", 1.0, 1),
    ];
    document.source = tableau(
        "/ab/",
        constraints.clone(),
        vec![
            candidate("ab", "[ab]", &[0, 0]),
            candidate("ap", "[ap]", &[1, 0]),
            candidate("a", "[a]", &[0, 1]),
        ],
    );
    document.source.source_locator = "Dissertation EN/PT-BR; fig:intro-sot-opening; panel:intro-neutral-source; fidelity:exact-transform (candidate and constraint labels reparameterized)".to_owned();
    let mut target_constraints = vec![constraints[1].clone(), constraints[0].clone()];
    target_constraints[0].stratum = 0;
    target_constraints[1].stratum = 1;
    document.target = tableau(
        "/ab/",
        target_constraints,
        vec![
            candidate("ab", "[ab]", &[0, 0]),
            candidate("ap", "[ap]", &[0, 1]),
            candidate("a", "[a]", &[1, 0]),
        ],
    );
    document.target.source_locator = "Dissertation EN/PT-BR; fig:intro-sot-opening; encoded target response; fidelity:exact-transform (candidate and constraint labels reparameterized)".to_owned();
    document.second_order.query = QueryKind::CompleteOrder;
    document.second_order.answer_sort = "preorder on all registered candidates".to_owned();
    document.second_order.scope = "complete order of ab, ap, and a".to_owned();
    document.second_order.transformation = "rerank IDENT-IO(voice) and MAX-IO".to_owned();
    document.second_order.transport = "identity on ab, ap, and a".to_owned();
    document.second_order.layout = SecondOrderLayout::Overlay;
    document.dataset = vec![document.source.clone()];
    document
}

/// Anttila-style Finnish variation used for the exact clone audit.
pub fn finnish_ranking_space() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Finnish weak and strong genitives".to_owned();
    document.source = tableau(
        "/naapuri/",
        vec![
            constraint("*H/I", 1.0, 0),
            constraint("*Í", 1.0, 0),
            constraint("*L.L", 1.0, 0),
        ],
        vec![
            candidate("weak", "naapurien", &[0, 0, 1]),
            candidate("strong", "naapureiden", &[1, 1, 0]),
        ],
    );
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document
}

fn dissertation_tableau(
    name: &str,
    input: &str,
    locator: &str,
    evaluator: EvaluatorKind,
    constraints: &[(&str, f64, usize)],
    candidates: &[(&str, &[u16], f64)],
    expected: &[&str],
) -> Tableau {
    let mut result = tableau(
        input,
        constraints
            .iter()
            .map(|(name, weight, stratum)| constraint(name, *weight, *stratum))
            .collect(),
        candidates
            .iter()
            .map(|(name, violations, mass)| {
                let mut item = candidate(name, name, violations);
                item.base_mass = scalar(*mass);
                item
            })
            .collect(),
    );
    result.name = name.to_owned();
    result.id = format!("dissertation:{name}");
    result.evaluator = Some(evaluator);
    result.expected_winners = expected.iter().map(|name| (*name).to_owned()).collect();
    result.source_locator = locator.to_owned();
    result.notes = "Transcribed regression fixture. PhonoScript GUI calculates the result from the stored constraint and candidate records before comparing it with the declared expectation.".to_owned();
    result
}

/// The bounded dissertation transcription used by the regression corpus.
/// Multi-block records are expanded into independently evaluable tableaux;
/// every record carries a stable LaTeX label, a within-display fragment, and
/// an explicit fidelity ceiling shared by both dissertation languages.
pub fn dissertation_project() -> ConvalgenDocument {
    use EvaluatorKind::{HarmonicGrammar as Hg, MaxEnt, Ot};
    let mut document = ConvalgenDocument::blank();
    document.title = "Dissertation tableau validation project".to_owned();
    document.author = "Alexandre Menezes Barroso".to_owned();
    document.description = "A bounded transcription of dissertation tableau records, expanded at input and stage boundaries and anchored to stable EN/PT-BR LaTeX labels. Stored expectations are regression oracles only: the native evaluator calculates each answer independently. Each locator declares whether the record is exact, transformed, neutral, a snapshot, a flattened panel, or source-panel-only. Custom-score records use declared order-preserving integer costs, and neutral records without a declared evaluator have no winner oracle.".to_owned();
    document.keywords = vec![
        "dissertation".to_owned(),
        "regression".to_owned(),
        "OT".to_owned(),
        "HG".to_owned(),
        "MaxEnt".to_owned(),
        "Second-Order Tableau".to_owned(),
    ];
    let mut cases = vec![
        dissertation_tableau(
            "H.1 Neutral HG selection",
            "/x/",
            "Dissertation EN/PT-BR; fig:apph-neutral-selection-tableau; sec:apph-pairwise-versus-selection; fidelity:exact",
            Hg,
            &[("C1", 2.0, 0), ("C2", 1.0, 1)],
            &[
                ("a", &[0, 0], 1.0),
                ("b", &[1, 0], 1.0),
                ("c", &[0, 1], 1.0),
            ],
            &["a"],
        ),
        dissertation_tableau(
            "H.2 Goldrick-Daland x score replay",
            "x=(1,0)",
            "Dissertation EN/PT-BR; tab:apph-gd-score-replay; block:apph-gd-input-x; eq:apph-gd-score-rows; fidelity:exact-transform (cost=20-H)",
            Hg,
            &[("order-preserving cost", 1.0, 0)],
            &[
                ("0", &[20], 1.0),
                ("y", &[0], 1.0),
                ("u", &[37], 1.0),
                ("z", &[19], 1.0),
            ],
            &["y"],
        ),
        dissertation_tableau(
            "H.3 Goldrick-Daland w score replay",
            "w=(0,1)",
            "Dissertation EN/PT-BR; tab:apph-gd-score-replay; block:apph-gd-input-w; eq:apph-gd-score-rows; fidelity:exact-transform (cost=18-H)",
            Hg,
            &[("order-preserving cost", 1.0, 0)],
            &[
                ("0", &[18], 1.0),
                ("y", &[15], 1.0),
                ("u", &[1], 1.0),
                ("z", &[0], 1.0),
            ],
            &["z"],
        ),
        dissertation_tableau(
            "H.4 Exact tenths-grid objective",
            "x=0,...,1",
            "Dissertation EN/PT-BR; tab:apph-mccollum-grid; sec:apph-mccollum; fidelity:exact-transform (100 J1(x))",
            Hg,
            &[("100 J1(x)", 1.0, 0)],
            &[
                ("x=0", &[2100], 1.0),
                ("x=1/10", &[1711], 1.0),
                ("x=1/5", &[1364], 1.0),
                ("x=3/10", &[1059], 1.0),
                ("x=2/5", &[796], 1.0),
                ("x=1/2", &[575], 1.0),
                ("x=3/5", &[396], 1.0),
                ("x=7/10", &[259], 1.0),
                ("x=4/5", &[164], 1.0),
                ("x=9/10", &[111], 1.0),
                ("x=1", &[100], 1.0),
            ],
            &["x=1"],
        ),
    ];
    let basic_constraints = [
        ("Onset", 1.0, 0),
        ("NoCoda", 1.0, 1),
        ("Dep", 1.0, 2),
        ("Max", 1.0, 3),
    ];
    cases.extend([
        dissertation_tableau(
            "H.5 Basic Syllable /CV/",
            "/CV/",
            "Dissertation EN/PT-BR; tab:apph-basic-tensor; block:/CV/; fidelity:neutral-ledger",
            Ot,
            &basic_constraints,
            &[
                ("CV", &[0, 0, 0, 0], 1.0),
                ("CVC", &[0, 1, 1, 0], 1.0),
                ("V", &[1, 0, 0, 1], 1.0),
                ("VC", &[1, 1, 1, 1], 1.0),
            ],
            &[],
        ),
        dissertation_tableau(
            "H.6 Basic Syllable /CVC/",
            "/CVC/",
            "Dissertation EN/PT-BR; tab:apph-basic-tensor; block:/CVC/; fidelity:neutral-ledger",
            Ot,
            &basic_constraints,
            &[
                ("CV", &[0, 0, 0, 1], 1.0),
                ("CVC", &[0, 1, 0, 0], 1.0),
                ("V", &[1, 0, 0, 2], 1.0),
                ("VC", &[1, 1, 0, 1], 1.0),
            ],
            &[],
        ),
        dissertation_tableau(
            "H.7 Basic Syllable /V/",
            "/V/",
            "Dissertation EN/PT-BR; tab:apph-basic-tensor; block:/V/; fidelity:neutral-ledger",
            Ot,
            &basic_constraints,
            &[
                ("CV", &[0, 0, 1, 0], 1.0),
                ("CVC", &[0, 1, 2, 0], 1.0),
                ("V", &[1, 0, 0, 0], 1.0),
                ("VC", &[1, 1, 1, 0], 1.0),
            ],
            &[],
        ),
        dissertation_tableau(
            "H.8 Basic Syllable /VC/",
            "/VC/",
            "Dissertation EN/PT-BR; tab:apph-basic-tensor; block:/VC/; fidelity:neutral-ledger",
            Ot,
            &basic_constraints,
            &[
                ("CV", &[0, 0, 1, 1], 1.0),
                ("CVC", &[0, 1, 1, 0], 1.0),
                ("V", &[1, 0, 0, 1], 1.0),
                ("VC", &[1, 1, 0, 0], 1.0),
            ],
            &[],
        ),
    ]);
    let walker = [
        (
            "H.9 Walker source 1",
            15_u16,
            18_u16,
            "assimilation",
            "id 1; fidelity:exact",
        ),
        (
            "H.10 Walker source 2",
            15,
            20,
            "assimilation",
            "id 2; fidelity:exact",
        ),
        (
            "H.11 Walker source 3",
            17,
            16,
            "faithful",
            "id 3; fidelity:exact",
        ),
        (
            "H.12 Walker source 4",
            17,
            20,
            "assimilation",
            "id 4; fidelity:exact",
        ),
        (
            "H.13 Walker source 5",
            19,
            16,
            "faithful",
            "id 5; fidelity:exact",
        ),
        (
            "H.14 Walker source 6",
            19,
            18,
            "faithful",
            "id 6; fidelity:exact",
        ),
        (
            "H.15 Walker interior witness",
            170,
            176,
            "assimilation",
            "id 7 new witness; fidelity:exact-transform (10P)",
        ),
        (
            "H.16 Walker boundary",
            17,
            17,
            "assimilation",
            "id 8 boundary; fidelity:exact",
        ),
    ];
    for (name, assimilation, faithful, expected, fragment) in walker {
        let expectation: &[&str] = if assimilation == faithful {
            &["assimilation", "faithful"]
        } else {
            &[expected]
        };
        let locator = format!("Dissertation EN/PT-BR; tab:apph-walker-replay; {fragment}");
        cases.push(dissertation_tableau(
            name,
            "Walker cell",
            &locator,
            Hg,
            &[("printed cost", 1.0, 0)],
            &[
                ("assimilation", &[assimilation], 1.0),
                ("faithful", &[faithful], 1.0),
            ],
            expectation,
        ));
    }
    cases.extend([
        dissertation_tableau(
            "H.17 Hidden-candidate MaxEnt fibre",
            "one hidden-candidate input",
            "Dissertation EN/PT-BR; fig:apph-pater-scaling-tableau; panel:apph-pater-source; fidelity:snapshot-only (t=log 2 represented by base masses)",
            MaxEnt,
            &[("score", 0.0, 0)],
            &[
                ("a->A", &[0], 1.0),
                ("b->A", &[0], 1.0),
                ("c->A", &[0], 1.0),
                ("d->B", &[0], 2.0),
            ],
            &["d->B"],
        ),
        dissertation_tableau(
            "H.18 One-shot support",
            "n=0,...,5",
            "Dissertation EN/PT-BR; fig:apph-cabrera-consumer-tableaux; panel:apph-cabrera-one-shot; eq:apph-cabrera-one-shot; fidelity:exact",
            MaxEnt,
            &[("Markedness", 0.5, 0)],
            &[
                ("c0", &[0], 1.0),
                ("c1", &[1], 1.0),
                ("c2", &[2], 1.0),
                ("c3", &[3], 1.0),
                ("c4", &[4], 1.0),
                ("c5", &[5], 1.0),
            ],
            &["c0"],
        ),
        dissertation_tableau(
            "H.19 Binary MParse boundary",
            "n=3",
            "Dissertation EN/PT-BR; fig:apph-cabrera-consumer-tableaux; panel:apph-cabrera-binary; eq:apph-cabrera-binary; fidelity:exact",
            MaxEnt,
            &[("Markedness", 0.5, 0), ("MParse", 1.5, 1)],
            &[("c3", &[3, 0], 1.0), ("null", &[0, 1], 1.0)],
            &["c3", "null"],
        ),
        dissertation_tableau(
            "E.1 Smallest MaxEnt tableau",
            "/u/",
            "Dissertation EN/PT-BR; fig:appe-smallest-maxent; sec:app-maxent-cross-product; fidelity:exact-transform (w=1 specialization)",
            MaxEnt,
            &[("C1", 1.0, 0)],
            &[("a", &[0], 1.0), ("b", &[1], 1.0)],
            &["a"],
        ),
        dissertation_tableau(
            "E.2 Polynomial ledger /u/",
            "/u/",
            "Dissertation EN/PT-BR; fig:appe-polynomial-ledgers; block:/u/; eq:app-maxent-compiler; fidelity:exact-transform (w=1 specialization)",
            MaxEnt,
            &[("C", 1.0, 0)],
            &[("a", &[0], 1.0), ("b1", &[0], 1.0), ("b2", &[2], 1.0)],
            &["a", "b1"],
        ),
        dissertation_tableau(
            "E.3 Polynomial ledger /u'/",
            "/u'/",
            "Dissertation EN/PT-BR; fig:appe-polynomial-ledgers; block:/u-prime/; eq:app-maxent-compiler; fidelity:exact-transform (w=1 specialization)",
            MaxEnt,
            &[("C", 1.0, 0)],
            &[("a-prime", &[0], 1.0), ("c1", &[1], 1.0), ("c2", &[1], 1.0)],
            &["a-prime"],
        ),
    ]);
    let simple = [
        (
            "1.1 Neutral profile",
            "Dissertation EN/PT-BR; fig:intro-neutral-profiles; sec:intro-opening; fidelity:neutral-ledger (input renamed /x/ to /u/)",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![0, 1]), ("c", vec![1, 0])],
            vec![],
        ),
        (
            "1.2 C1 above C2",
            "Dissertation EN/PT-BR; fig:intro-ranking-one; panel:intro-ranking-one; fidelity:exact-transform (input renamed)",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![0, 1]), ("c", vec![1, 0])],
            vec!["a"],
        ),
        (
            "1.3 C2 above C1",
            "Dissertation EN/PT-BR; fig:intro-ranking-two; panel:intro-ranking-two; fidelity:exact-transform (input renamed)",
            vec![("C2", 1.0, 0), ("C1", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![1, 0]), ("c", vec![0, 1])],
            vec!["a"],
        ),
        (
            "1.4 Candidate-deletion source",
            "Dissertation EN/PT-BR; fig:intro-deletion-sot; panel:intro-delete-source; fidelity:exact-transform (input renamed)",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![0, 1]), ("c", vec![1, 0])],
            vec!["a"],
        ),
        (
            "1.5 Neutral source",
            "Dissertation EN/PT-BR; fig:intro-sot-opening; panel:intro-neutral-source; fidelity:exact-transform (input renamed)",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![0, 1]), ("c", vec![1, 0])],
            vec!["a"],
        ),
        (
            "5.1 Prior-art C1 first",
            "Dissertation EN/PT-BR; tab:prior-neutral-ot; panel:prior-neutral-c1-first; fidelity:exact",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 1]), ("b", vec![1, 0]), ("c", vec![1, 1])],
            vec!["a"],
        ),
        (
            "5.2 Prior-art C2 first",
            "Dissertation EN/PT-BR; tab:prior-neutral-ot; panel:prior-neutral-c2-first; fidelity:exact",
            vec![("C2", 1.0, 0), ("C1", 1.0, 1)],
            vec![("a", vec![1, 0]), ("b", vec![0, 1]), ("c", vec![1, 1])],
            vec!["b"],
        ),
        (
            "5.3 Evaluator-neutral rows",
            "Dissertation EN/PT-BR; fig:prior-evaluator-neutral; sec:prior-evaluators; fidelity:neutral-ledger",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 1]), ("b", vec![1, 0])],
            vec![],
        ),
        (
            "4.1 MaxEnt-opening neutral rows",
            "Dissertation EN/PT-BR; fig:maxent-opening-profiles; sec:maxent-from-winner; fidelity:neutral-ledger",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 1]), ("b", vec![1, 0]), ("c", vec![1, 1])],
            vec![],
        ),
        (
            "2.1 Calculus neutral source",
            "Dissertation EN/PT-BR; fig:calc-ex01-sot; panel:calc2-neutral-source; fidelity:exact-transform (input renamed)",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1)],
            vec![("a", vec![0, 0]), ("b", vec![0, 1]), ("c", vec![1, 0])],
            vec!["a"],
        ),
        (
            "2.4 Four-question source",
            "Dissertation EN/PT-BR; tab:calc-four-question-matrix; panel:calc-four-question-source; eq:calc-four-question-source-order; fidelity:exact",
            vec![("C1", 1.0, 0), ("C2", 1.0, 1), ("C3", 1.0, 2)],
            vec![
                ("a", vec![0, 0, 0]),
                ("b", vec![0, 1, 0]),
                ("c", vec![0, 0, 1]),
                ("d", vec![1, 0, 0]),
            ],
            vec!["a"],
        ),
        (
            "2.5 Four-question target",
            "Dissertation EN/PT-BR; tab:calc-four-question-matrix; panel:calc-four-question-target; eq:calc-four-question-target-order; fidelity:exact",
            vec![("C1", 1.0, 0), ("C3", 1.0, 1), ("C2", 1.0, 2)],
            vec![
                ("a", vec![0, 0, 0]),
                ("b", vec![0, 0, 1]),
                ("c", vec![0, 1, 0]),
                ("d", vec![1, 0, 0]),
            ],
            vec!["a"],
        ),
    ];
    for (name, locator, constraints, candidates, expected) in simple {
        let c = constraints
            .iter()
            .map(|(n, w, s)| (*n, *w, *s))
            .collect::<Vec<_>>();
        let rows = candidates
            .iter()
            .map(|(n, v)| (*n, v.as_slice(), 1.0))
            .collect::<Vec<_>>();
        cases.push(dissertation_tableau(
            name, "/u/", locator, Ot, &c, &rows, &expected,
        ));
    }
    cases.extend([
        dissertation_tableau(
            "5.4 Neutral merger",
            "/u/",
            "Dissertation EN/PT-BR; fig:prior-neutral-merger; sec:prior-strongest-rivals; fidelity:snapshot-only (derived equal-weight candidate tie)",
            MaxEnt,
            &[("C1", 1.0, 0), ("C2", 1.0, 1)],
            &[
                ("a", &[0, 1], 1.0),
                ("b", &[1, 0], 1.0),
                ("c", &[1, 0], 1.0),
            ],
            &["a", "b", "c"],
        ),
        dissertation_tableau(
            "2.2 Serial source path",
            "neutral source",
            "Dissertation EN/PT-BR; fig:calc-stacked-serial-sot; panel:calc2-serial-source; fidelity:flattened-panel (not a serial derivation replay)",
            Ot,
            &[("C1", 1.0, 0), ("C2", 1.0, 1)],
            &[
                ("a", &[1, 1], 1.0),
                ("b", &[0, 1], 1.0),
                ("d", &[0, 0], 1.0),
            ],
            &["d"],
        ),
        dissertation_tableau(
            "2.3 Serial target path",
            "neutral target",
            "Dissertation EN/PT-BR; fig:calc-stacked-serial-sot; panel:calc2-serial-target; fidelity:flattened-panel (not a serial derivation replay)",
            Ot,
            &[("C1", 1.0, 0), ("C2", 1.0, 1)],
            &[
                ("a", &[1, 1], 1.0),
                ("c", &[1, 0], 1.0),
                ("d", &[0, 0], 1.0),
            ],
            &["d"],
        ),
        dissertation_tableau(
            "2.6 Merger source fibre",
            "source fibre",
            "Dissertation EN/PT-BR; fig:calc-ex09-sot; panel:calc2-merger-source; eq:calc-merger-transport; fidelity:source-panel-only (not the merger comparison)",
            MaxEnt,
            &[("Faith", 0.0, 0)],
            &[("a1", &[0], 1.0), ("a2", &[0], 1.0), ("b", &[0], 1.0)],
            &["a1", "a2", "b"],
        ),
        dissertation_tableau(
            "2.7 Refusal source fibre",
            "source fibre",
            "Dissertation EN/PT-BR; fig:calc-ex10-sot; panel:calc2-refusal-source; query:calc2-merger-refusal; fidelity:source-panel-only (not the structured refusal)",
            MaxEnt,
            &[("Faith", 0.0, 0)],
            &[("a1", &[0], 1.0), ("a2", &[0], 1.0), ("b", &[0], 1.0)],
            &["a1", "a2", "b"],
        ),
    ]);
    document.dataset = cases;
    document.source = dissertation_second_order().source;
    document.target = dissertation_second_order().target;
    document.second_order = dissertation_second_order().second_order;
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::evaluate;

    #[test]
    fn goldwater_johnson_table_two_is_an_eleven_constraint_transcription() {
        let project = goldwater_johnson_finnish_ledger();
        assert_eq!(project.dataset.len(), 4);
        assert!(project.dataset.iter().all(|tableau| {
            tableau.constraints.len() == 11
                && tableau.candidates.len() == 2
                && tableau.source_locator.contains("Table 2")
        }));
        assert_eq!(
            project.dataset[0].candidates[0].violations,
            [1, 1, 0, 0, 0, 0, 0, 1, 0, 1, 1]
        );
        assert_eq!(
            project.dataset[1].candidates[1].violations,
            [0, 1, 1, 0, 0, 0, 0, 0, 1, 0, 0]
        );
        let differences = project
            .dataset
            .iter()
            .map(|tableau| {
                tableau.candidates[1]
                    .violations
                    .iter()
                    .zip(&tableau.candidates[0].violations)
                    .map(|(strong, weak)| i32::from(*strong) - i32::from(*weak))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(differences[0], [0, 1, 0, 0, 0, 0, 0, 0, 1, -1, 0]);
        assert_eq!(differences[1], [0, 0, 1, 0, 0, -1, 0, 0, 1, -1, -2]);
        assert_eq!(differences[1], differences[2]);
        assert_eq!(differences[3], [0, 0, 0, 0, 1, 0, 0, -1, 2, 0, -2]);
    }

    #[test]
    fn dissertation_project_checks_every_internal_record_and_declared_oracle() {
        let project = dissertation_project();
        assert_eq!(project.dataset.len(), 39);
        assert!(
            project
                .dataset
                .iter()
                .all(|tableau| !tableau.source_locator.is_empty())
        );
        let mut checked = 0;
        for tableau in &project.dataset {
            if tableau.expected_winners.is_empty() {
                continue;
            }
            let result = evaluate(
                tableau,
                tableau.evaluator_or(project.evaluator),
                tableau.temperature_or(&project.temperature),
            );
            let mut actual = result
                .winner_indices
                .iter()
                .map(|index| tableau.candidates[*index].name.clone())
                .collect::<Vec<_>>();
            actual.sort();
            let mut expected = tableau.expected_winners.clone();
            expected.sort();
            assert_eq!(
                actual, expected,
                "{} ({})",
                tableau.name, tableau.source_locator
            );
            checked += 1;
        }
        assert_eq!(checked, 32);
    }
}
