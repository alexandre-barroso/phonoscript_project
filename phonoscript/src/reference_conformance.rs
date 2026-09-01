//! Source-bounded conformance fixtures for published phonological analyses.
//!
//! Every fixture carries a source key, a page/tableau locator, and an explicit
//! claim ceiling.  These are scholarly regression records: they never turn an
//! unprinted weight, hidden violation, unrestricted GEN, or probability law
//! into an implementation default.

use crate::exact::NumericScalar;
use crate::model::{Candidate, Constraint, ConvalgenDocument, SerialMove, SerialSettings, Tableau};
use crate::ranking::PartialRanking;

#[derive(Debug, Clone)]
pub struct LocatedTableau {
    pub source_key: &'static str,
    pub locator: &'static str,
    pub claim_ceiling: &'static str,
    pub tableau: Tableau,
}

#[derive(Debug, Clone)]
pub struct AnttilaCompetition {
    pub label: &'static str,
    pub expected_counts: [u32; 2],
    pub tableau: Tableau,
}

#[derive(Debug, Clone)]
pub struct AnttilaChoFixture {
    pub source_key: &'static str,
    pub locator: &'static str,
    pub claim_ceiling: &'static str,
    pub partial_ranking: PartialRanking,
    pub competitions: Vec<AnttilaCompetition>,
}

#[derive(Debug, Clone)]
pub struct RimiFixture {
    pub source_key: &'static str,
    pub parallel_locator: &'static str,
    pub serial_locator: &'static str,
    pub claim_ceiling: &'static str,
    pub parallel: Tableau,
    pub serial_tableau: Tableau,
    pub serial: SerialSettings,
}

#[derive(Debug, Clone)]
pub struct GoldwaterJohnsonFixture {
    pub source_key: &'static str,
    pub ledger_locator: &'static str,
    pub difference_locator: &'static str,
    pub probability_report_locator: &'static str,
    pub claim_ceiling: &'static str,
    pub ledger: ConvalgenDocument,
    pub expected_strong_minus_weak: [[i16; 11]; 4],
    pub reported_maxent_percent: [f64; 8],
}

fn constraint(name: &str, weight: f64, stratum: usize) -> Constraint {
    Constraint {
        id: format!("constraint:{name}"),
        name: name.to_owned(),
        weight: Some(
            NumericScalar::parse_exact(&weight.to_string())
                .expect("reference weight has a finite decimal spelling"),
        ),
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

fn tableau(
    name: &str,
    input: &str,
    constraints: Vec<Constraint>,
    candidates: Vec<Candidate>,
    locator: &str,
) -> Tableau {
    Tableau {
        id: format!("tableau:{name}"),
        name: name.to_owned(),
        input: input.to_owned(),
        constraints,
        candidates,
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: None,
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: Vec::new(),
        source_locator: locator.to_owned(),
    }
}

/// Kager's two exact final-voicing tableaux use the same candidates and marks
/// with the constraint ranking reversed.
pub fn kager_dutch_english_final_voicing() -> [LocatedTableau; 2] {
    let locator_dutch = "Kager 1999, printed p. 16, Tableau (18), physical PDF p. 32";
    let locator_english = "Kager 1999, printed p. 17, Tableau (23), physical PDF p. 33";
    let dutch = tableau(
        "Dutch final devoicing",
        "/bɛd/",
        vec![
            constraint("*VOICED-CODA", 1.0, 0),
            constraint("IDENT-IO(voice)", 1.0, 1),
        ],
        vec![
            candidate("devoiced", "[bɛt]", &[0, 1]),
            candidate("faithful", "[bɛd]", &[1, 0]),
        ],
        locator_dutch,
    );
    let english = tableau(
        "English final voicing preservation",
        "/bɛd/",
        vec![
            constraint("*VOICED-CODA", 1.0, 1),
            constraint("IDENT-IO(voice)", 1.0, 0),
        ],
        vec![
            candidate("devoiced", "[bɛt]", &[0, 1]),
            candidate("faithful", "[bɛd]", &[1, 0]),
        ],
        locator_english,
    );
    [
        LocatedTableau {
            source_key: "kager1999optimality",
            locator: locator_dutch,
            claim_ceiling: "Exact two-candidate displayed fragment; not Kager's unrestricted GEN or a complete Dutch grammar.",
            tableau: dutch,
        },
        LocatedTableau {
            source_key: "kager1999optimality",
            locator: locator_english,
            claim_ceiling: "Exact two-candidate displayed fragment; not Kager's unrestricted GEN or a complete English grammar.",
            tableau: english,
        },
    ]
}

/// Both panels of Pater's printed positional-faithfulness HG tableau.
pub fn pater_positional_faithfulness_panels() -> [LocatedTableau; 2] {
    let locator = "Pater 2008, Tableau (13), physical PDF p. 8";
    let constraints = || {
        vec![
            constraint("*VOICE", 1.5, 0),
            constraint("IDENT-VOICE-ONSET", 1.0, 1),
            constraint("IDENT-VOICE", 1.0, 2),
        ]
    };
    [
        LocatedTableau {
            source_key: "pater2008gradient",
            locator,
            claim_ceiling: "Exact finite HG panel and printed weights; no claim about the later learning simulation.",
            tableau: tableau(
                "Pater HG /da/ panel",
                "/da/",
                constraints(),
                vec![
                    candidate("devoiced", "[ta]", &[0, 1, 1]),
                    candidate("faithful", "[da]", &[1, 0, 0]),
                ],
                locator,
            ),
        },
        LocatedTableau {
            source_key: "pater2008gradient",
            locator,
            claim_ceiling: "Exact finite HG panel and printed weights; no claim about the later learning simulation.",
            tableau: tableau(
                "Pater HG /tad/ panel",
                "/tad/",
                constraints(),
                vec![
                    candidate("faithful", "[tad]", &[1, 0, 0]),
                    candidate("devoiced", "[tat]", &[0, 0, 1]),
                ],
                locator,
            ),
        },
    ]
}

/// Tessier's shared `/skul/` HG/MaxEnt ledger. The source prints the four
/// weights, all three violation vectors, and the Harmony scores. Its displayed
/// decimal for `exp(-11)` is off by one decimal place and its MaxEnt column is
/// unnormalized, so conformance means recovering the printed costs and then
/// calculating the corrected finite conditional probabilities.
pub fn tessier_skul_hg_maxent() -> LocatedTableau {
    let locator = "Tessier 2017, physical PDF p. 16, Tableaux (14)-(15), /skul/ panels";
    LocatedTableau {
        source_key: "tessier2017learnability",
        locator,
        claim_ceiling: "Exact for the displayed three-candidate support, four printed weights, violation ledger, and Harmony scores. The normalized MaxEnt probabilities are the project's correction of the source's unnormalized values and erroneous decimal for exp(-11), not a learned-weight or complete-GEN claim.",
        tableau: tableau(
            "Tessier /skul/ shared HG-MaxEnt ledger",
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
            locator,
        ),
    }
}

/// Anttila and Cho's English root grammar `*CODA >> ONSET` and the four
/// candidate pairs counted in their Table (11).  Constraint indices are
/// `*CODA=0`, `ONSET=1`, `FAITH=2`.
pub fn anttila_cho_linking_r() -> AnttilaChoFixture {
    let locator = "Anttila & Cho 1998, printed pp. 35-39, grammar lattice (10), Table (11), Eq. (12); physical PDF pp. 5-9";
    let constraints = || {
        vec![
            constraint("*CODA", 1.0, 0),
            constraint("ONSET", 1.0, 1),
            constraint("FAITH", 1.0, 2),
        ]
    };
    let competition =
        |label, input, left, right, left_marks, right_marks, expected_counts| AnttilaCompetition {
            label,
            expected_counts,
            tableau: tableau(
                label,
                input,
                constraints(),
                vec![
                    candidate("candidate-a", left, left_marks),
                    candidate("candidate-b", right, right_marks),
                ],
                locator,
            ),
        };
    AnttilaChoFixture {
        source_key: "anttilaCho1998variationChange",
        locator,
        claim_ceiling: "Exact for the three English total subgrammars under the source's uniform-tableau interpretation in Eq. (12); it is not an invariant probability under arbitrary ranking measures, constraint aliases, or candidate cloning.",
        partial_ranking: PartialRanking {
            constraint_names: vec!["*CODA".to_owned(), "ONSET".to_owned(), "FAITH".to_owned()],
            dominance: vec![(0, 1)],
        },
        competitions: vec![
            competition(
                "Wanda left",
                "Wanda left",
                "Wanda left",
                "Wanda[r] left",
                &[0, 0, 0],
                &[1, 0, 1],
                [3, 0],
            ),
            competition(
                "Homer left",
                "Homer left",
                "Homer left",
                "Home<r> left",
                &[1, 0, 0],
                &[0, 0, 1],
                [1, 2],
            ),
            competition(
                "Wanda arrived",
                "Wanda arrived",
                "Wanda arrived",
                "Wanda[r] arrived",
                &[0, 1, 0],
                &[0, 0, 1],
                [2, 1],
            ),
            competition(
                "Homer arrived",
                "Homer arrived",
                "Homer arrived",
                "Home<r> arrived",
                &[0, 0, 0],
                &[0, 1, 1],
                [3, 0],
            ),
        ],
    }
}

/// McCarthy's Rimi evidence separated into (i) an exact two-row fragment of
/// the parallel Tableau (17), and (ii) the bounded spreading-first GEN1
/// projection in (18).  Shaded, unreported lower cells are never invented.
pub fn mccarthy_rimi_parallel_and_gen1() -> RimiFixture {
    let parallel_locator =
        "McCarthy 2000, printed p. 517, Tableau (17), physical PDF p. 18, rows a-b";
    let serial_locator = "McCarthy 2000, printed p. 518, derivation (18), physical PDF p. 19";
    let parallel = tableau(
        "Rimi tone flop: parallel selected-row fragment",
        "/rá-muntu/",
        vec![
            constraint("NO-LONG-T", 1.0, 0),
            constraint("LOCAL", 1.0, 0),
            constraint("MAX(T)", 1.0, 0),
            constraint("ALIGN-R", 1.0, 1),
            constraint("MAX(A)", 1.0, 2),
            constraint("DEP(A)", 1.0, 2),
        ],
        vec![
            candidate("faithful-prefix-H", "rámuntu", &[0, 0, 0, 2, 0, 0]),
            candidate("tone-flop", "ramúntu", &[0, 0, 0, 1, 1, 1]),
        ],
        parallel_locator,
    );

    let serial_tableau = tableau(
        "Rimi spreading-first GEN1 projection",
        "A: prefix-linked H",
        vec![
            constraint("NO-LONG-T", 1.0, 0),
            constraint("ALIGN-R", 1.0, 1),
        ],
        vec![
            candidate("identity-A", "A: prefix-linked H", &[0, 2]),
            candidate("spread-B", "B: multiply-linked H", &[1, 1]),
        ],
        serial_locator,
    );
    let a = "A: prefix-linked H";
    let b = "B: multiply-linked H";
    let c = "C: root-linked H";
    let serial = SerialSettings {
        start: a.to_owned(),
        moves: vec![
            SerialMove {
                from: a.to_owned(),
                to: a.to_owned(),
                operation: "identity".to_owned(),
                violations: vec![0, 2],
            },
            SerialMove {
                from: a.to_owned(),
                to: b.to_owned(),
                operation: "insert one association line (spread)".to_owned(),
                violations: vec![1, 1],
            },
            SerialMove {
                from: b.to_owned(),
                to: b.to_owned(),
                operation: "identity".to_owned(),
                violations: vec![1, 1],
            },
            SerialMove {
                from: b.to_owned(),
                to: c.to_owned(),
                operation: "delete one association line (delink)".to_owned(),
                violations: vec![0, 1],
            },
            SerialMove {
                from: c.to_owned(),
                to: c.to_owned(),
                operation: "identity".to_owned(),
                violations: vec![0, 1],
            },
        ],
        maximum_steps: 8,
    };
    RimiFixture {
        source_key: "mccarthy2000harmonic",
        parallel_locator,
        serial_locator,
        claim_ceiling: "The parallel assertion is exact only for printed rows a-b. The serial assertion is the source's two-constraint spreading-first path obstruction, not a complete Rimi grammar and not a claim about later Harmonic Serialism.",
        parallel,
        serial_tableau,
        serial,
    }
}

/// The exact Finnish candidate ledger and integer difference table printed by
/// Goldwater and Johnson, together with their rounded probability report.
/// The fitted Finnish weights are not printed and are deliberately absent
/// from the replayable part of this fixture.
pub fn goldwater_johnson_finnish_report() -> GoldwaterJohnsonFixture {
    GoldwaterJohnsonFixture {
        source_key: "goldwater2003learning",
        ledger_locator: "Goldwater & Johnson 2003, Table 2, physical PDF p. 6",
        difference_locator: "Goldwater & Johnson 2003, Table 3, physical PDF p. 6",
        probability_report_locator: "Goldwater & Johnson 2003, Table 4, physical PDF p. 6",
        claim_ceiling: "Exact for the Table 2 violation ledger, Table 3 integer strong-minus-weak differences, and Table 4 rounded reported percentages. The learned Finnish weight vector is absent, so fitted-probability recomputation, learned-parameter recovery, and numerical cross-tool replay are not licensed.",
        ledger: crate::reference_cases::goldwater_johnson_finnish_ledger(),
        expected_strong_minus_weak: [
            [0, 1, 0, 0, 0, 0, 0, 0, 1, -1, 0],
            [0, 0, 1, 0, 0, -1, 0, 0, 1, -1, -2],
            [0, 0, 1, 0, 0, -1, 0, 0, 1, -1, -2],
            [0, 0, 0, 0, 1, 0, 0, -1, 2, 0, -2],
        ],
        reported_maxent_percent: [99.6, 100.0, 100.0, 69.4, 99.8, 98.0, 80.5, 55.3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ComparisonStatus;
    use crate::model::{EvaluatorKind, QueryKind};
    use crate::phonological_engine::{EngineStage, PhonologicalEngine};
    use crate::ranking::LinearExtensions;

    #[test]
    fn kager_reranking_reverses_the_exact_two_candidate_winner() {
        let engine = PhonologicalEngine::new();
        let [dutch, english] = kager_dutch_english_final_voicing();
        assert_eq!(dutch.tableau.source_locator, dutch.locator);
        assert_eq!(english.tableau.source_locator, english.locator);
        assert_eq!(
            engine
                .evaluate(&dutch.tableau, EvaluatorKind::Ot, 1.0)
                .expect("the printed Dutch tableau is formed")
                .winner_indices,
            [0]
        );
        assert_eq!(
            engine
                .evaluate(&english.tableau, EvaluatorKind::Ot, 1.0)
                .expect("the printed English tableau is formed")
                .winner_indices,
            [1]
        );
    }

    #[test]
    fn pater_both_panels_reproduce_the_printed_hg_costs_and_winners() {
        let engine = PhonologicalEngine::new();
        let [onset, coda] = pater_positional_faithfulness_panels();
        let onset_result = engine
            .evaluate(&onset.tableau, EvaluatorKind::HarmonicGrammar, 1.0)
            .expect("Pater's /da/ panel is formed");
        let coda_result = engine
            .evaluate(&coda.tableau, EvaluatorKind::HarmonicGrammar, 1.0)
            .expect("Pater's /tad/ panel is formed");
        assert_eq!(onset_result.winner_indices, [1]);
        assert_eq!(onset_result.rows[0].harmony, 2.0);
        assert_eq!(onset_result.rows[1].harmony, 1.5);
        assert_eq!(coda_result.winner_indices, [1]);
        assert_eq!(coda_result.rows[0].harmony, 1.5);
        assert_eq!(coda_result.rows[1].harmony, 1.0);
    }

    #[test]
    fn tessier_shared_ledger_reproduces_hg_and_corrects_maxent_normalization() {
        let engine = PhonologicalEngine::new();
        let fixture = tessier_skul_hg_maxent();
        assert_eq!(fixture.source_key, "tessier2017learnability");
        assert_eq!(fixture.tableau.source_locator, fixture.locator);

        let hg = engine
            .evaluate(&fixture.tableau, EvaluatorKind::HarmonicGrammar, 1.0)
            .expect("Tessier's printed HG ledger is formed");
        assert_eq!(hg.winner_indices, [1]);
        assert_eq!(hg.rows[0].harmony, 11.0);
        assert_eq!(hg.rows[1].harmony, 1.0);
        assert_eq!(hg.rows[2].harmony, 2.0);
        assert_eq!(
            hg.rows
                .iter()
                .map(|row| row.exact_harmony.as_ref().map(ToString::to_string))
                .collect::<Vec<_>>(),
            [
                Some("11".to_owned()),
                Some("1".to_owned()),
                Some("2".to_owned())
            ]
        );

        let maxent = engine
            .evaluate(&fixture.tableau, EvaluatorKind::MaxEnt, 1.0)
            .expect("Tessier's finite MaxEnt ledger is formed");
        assert_eq!(maxent.winner_indices, [1]);
        let expected = [
            0.000_033_188_906_581_985_21,
            0.731_034_315_595_132_8,
            0.268_932_495_498_285_24,
        ];
        for (row, expected) in maxent.rows.iter().zip(expected) {
            assert!((row.probability.expect("MaxEnt probability") - expected).abs() < 1.0e-15);
        }
        let odds = maxent.rows[1].probability.expect("winner probability")
            / maxent.rows[2].probability.expect("runner-up probability");
        assert!((odds - std::f64::consts::E).abs() < 1.0e-14);
    }

    #[test]
    fn anttila_root_partial_order_has_three_extensions_and_exact_event_counts() {
        let engine = PhonologicalEngine::new();
        let fixture = anttila_cho_linking_r();
        let orders = match engine.linear_extensions(&fixture.partial_ranking, 16) {
            LinearExtensions::Complete { orders } => orders,
            other => {
                panic!("the printed root grammar must have three complete extensions: {other:?}")
            }
        };
        assert_eq!(orders.len(), 3);
        for competition in fixture.competitions {
            assert_eq!(competition.tableau.source_locator, fixture.locator);
            let mut counts = [0_u32; 2];
            for order in &orders {
                let mut ranked = competition.tableau.clone();
                for (stratum, constraint_index) in order.iter().copied().enumerate() {
                    ranked.constraints[constraint_index].stratum = stratum;
                }
                let result = engine
                    .evaluate(&ranked, EvaluatorKind::Ot, 1.0)
                    .expect("each printed total subgrammar is formed");
                assert_eq!(result.winner_indices.len(), 1);
                counts[result.winner_indices[0]] += 1;
            }
            assert_eq!(counts, competition.expected_counts, "{}", competition.label);
        }
    }

    #[test]
    fn rimi_parallel_fragment_selects_flop_but_spreading_first_gen1_halts() {
        let engine = PhonologicalEngine::new();
        let fixture = mccarthy_rimi_parallel_and_gen1();
        assert_eq!(fixture.parallel.source_locator, fixture.parallel_locator);
        assert_eq!(
            fixture.serial_tableau.source_locator,
            fixture.serial_locator
        );
        assert_eq!(
            engine
                .evaluate(&fixture.parallel, EvaluatorKind::Ot, 1.0)
                .expect("the exact printed two-row parallel fragment is formed")
                .winner_indices,
            [1]
        );
        let serial = engine
            .serial(
                &fixture.serial_tableau,
                &fixture.serial,
                EvaluatorKind::Ot,
                1.0,
            )
            .expect("the bounded GEN1 projection is formed");
        assert!(serial.formed);
        assert_eq!(serial.path, ["A: prefix-linked H"]);
        assert_eq!(serial.stopped, "faithful convergence");
    }

    #[test]
    fn goldwater_johnson_tables_two_and_three_are_exact_but_table_four_is_not_replayed() {
        let engine = PhonologicalEngine::new();
        let fixture = goldwater_johnson_finnish_report();
        let document = &fixture.ledger;
        assert_eq!(document.dataset.len(), 4);
        for (tableau, expected) in document
            .dataset
            .iter()
            .zip(fixture.expected_strong_minus_weak)
        {
            assert_eq!(tableau.constraints.len(), 11);
            assert_eq!(
                tableau.source_locator,
                "Goldwater & Johnson 2003, Table 2, PDF physical page 6"
            );
            let weak = &tableau.candidates[0].violations;
            let strong = &tableau.candidates[1].violations;
            let actual = std::array::from_fn::<_, 11, _>(|index| {
                i16::try_from(strong[index]).expect("u16 mark fits i16")
                    - i16::try_from(weak[index]).expect("u16 mark fits i16")
            });
            assert_eq!(actual, expected);
        }

        // The Finnish fitted weight vector is absent from the article.  An
        // explicit unknown is rejected by the same checked engine rather than
        // becoming zero, NaN probability, or an invented uniform grammar.
        let unavailable = document.dataset[0].clone();
        let refusal = engine
            .evaluate(&unavailable, EvaluatorKind::MaxEnt, 1.0)
            .expect_err("an unprinted weight cannot be evaluated");
        assert_eq!(refusal.code, "PE-ADMIT-MISSING-FITTED-WEIGHTS");
        assert_eq!(refusal.stage, EngineStage::Admission);
        assert_eq!(refusal.coordinate, "constraints.fitted-weights");

        // Table 4 is a rounded report, preserved as source data only.
        assert_eq!(fixture.reported_maxent_percent[3], 69.4);
        assert_eq!(fixture.reported_maxent_percent[7], 55.3);
    }

    #[test]
    fn malformed_source_dependency_becomes_second_order_not_evaluated() {
        let engine = PhonologicalEngine::new();
        let mut project = ConvalgenDocument::blank();
        project.evaluator = EvaluatorKind::MaxEnt;
        project.second_order.query = QueryKind::ProbabilityLaw;
        project.source = goldwater_johnson_finnish_report().ledger.dataset[0].clone();
        project.target = project.source.clone();
        let result = engine.compare(&project);
        assert_eq!(result.status, ComparisonStatus::NotEvaluated);
        let refusal = result.refusal.expect("missing fitted weight is indexed");
        assert_eq!(refusal.code, "PE-ADMIT-MISSING-FITTED-WEIGHTS");
        assert_eq!(refusal.coordinate, "source.constraints.fitted-weights");
    }
}
