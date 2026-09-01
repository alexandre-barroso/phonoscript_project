use std::fs;
use std::path::{Path, PathBuf};

use convalgen::exact::NumericScalar;
use convalgen::export::{self, ExportFormat};
use convalgen::model::{
    Candidate, Constraint, ConvalgenDocument, EvaluatorKind, PlotKind, QueryKind,
    SecondOrderLayout, SerialMove, Tableau,
};
use convalgen::reference_cases;

fn scalar(value: &str) -> NumericScalar {
    NumericScalar::parse_exact(value).expect("visual fixture uses an exact numeric literal")
}

fn constraint(
    id: impl Into<String>,
    name: impl Into<String>,
    weight: &str,
    stratum: usize,
) -> Constraint {
    Constraint {
        id: id.into(),
        name: name.into(),
        weight: Some(scalar(weight)),
        stratum,
        enabled: true,
        definition: String::new(),
        prior_mean: NumericScalar::integer(0),
        prior_sigma: NumericScalar::integer(100_000),
    }
}

fn candidate(
    id: impl Into<String>,
    name: impl Into<String>,
    form: impl Into<String>,
    violations: Vec<u16>,
) -> Candidate {
    Candidate {
        id: id.into(),
        name: name.into(),
        form: form.into(),
        violations,
        base_mass: NumericScalar::integer(1),
        notes: String::new(),
        observed_frequency: NumericScalar::integer(0),
        structured: None,
    }
}

fn install_tableau(
    mut document: ConvalgenDocument,
    evaluator: EvaluatorKind,
    tableau: Tableau,
) -> ConvalgenDocument {
    document.evaluator = evaluator;
    document.source = tableau.clone();
    document.target = tableau.clone();
    document.dataset = vec![tableau];
    document
}

fn write_all(destination: &Path, name: &str, svg: &str, png_scale: f32) {
    let stem = destination.join(name);
    for format in ExportFormat::ALL {
        let path =
            export::write_with_scale(svg, &stem, format, png_scale).unwrap_or_else(|error| {
                panic!("could not export {name} as {}: {error}", format.label())
            });
        println!("{}", path.display());
    }
}

fn strict_tie_partial_order() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Tied stratum, independent boundary, and co-winners".to_owned();
    document.description =
        "Generic visual stress fixture; it is not attributed to a published analysis.".to_owned();
    document.presentation.show_legend = true;
    document.a_priori_rankings = vec![(0, 2)];
    let tableau = Tableau {
        id: "visual-strict-ties".to_owned(),
        name: "STRICT OT WITH DECLARED BOUNDARIES".to_owned(),
        input: "/x/".to_owned(),
        constraints: vec![
            constraint("tie-c1", "C1", "1", 0),
            constraint("tie-c2", "C2", "1", 0),
            constraint("partial-c3", "C3", "1", 1),
        ],
        candidates: vec![
            candidate("tie-a", "candidate a", "[a]", vec![0, 1, 0]),
            candidate("tie-b", "candidate b", "[b]", vec![1, 0, 0]),
            candidate("tie-c", "candidate c", "[c]", vec![1, 1, 6]),
        ],
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: Some(EvaluatorKind::Ot),
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: vec!["candidate a".to_owned(), "candidate b".to_owned()],
        source_locator: "Generic export stress fixture".to_owned(),
    };
    install_tableau(document, EvaluatorKind::Ot, tableau)
}

fn exact_hg_ties() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Exact HG tie with a rational near-tie".to_owned();
    document.description =
        "Generic exact-arithmetic export fixture; it is not attributed to a published analysis."
            .to_owned();
    let tableau = Tableau {
        id: "visual-hg-exact".to_owned(),
        name: "EXACT WEIGHTED COSTS".to_owned(),
        input: "/rational/".to_owned(),
        constraints: vec![
            constraint("hg-third", "C-third", "1/3", 0),
            constraint("hg-unit", "C-unit", "1", 1),
            constraint("hg-epsilon", "C-epsilon", "1/1000000", 2),
        ],
        candidates: vec![
            candidate("hg-a", "three thirds", "[aaa]", vec![3, 0, 0]),
            candidate("hg-b", "one unit", "[b]", vec![0, 1, 0]),
            candidate("hg-c", "near tie", "[c]", vec![3, 0, 1]),
        ],
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: Some(EvaluatorKind::HarmonicGrammar),
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: vec!["three thirds".to_owned(), "one unit".to_owned()],
        source_locator: "Generic exact-arithmetic export stress fixture".to_owned(),
    };
    install_tableau(document, EvaluatorKind::HarmonicGrammar, tableau)
}

fn nonuniform_maxent() -> ConvalgenDocument {
    let mut document = reference_cases::finite_maxent_smoke();
    document.title = "Finite MaxEnt with nonuniform base mass".to_owned();
    document.description = "Generic finite conditional MaxEnt smoke test; it is not attributed to a published tableau.".to_owned();
    document.source.candidates[0].base_mass = scalar("3/2");
    document.source.candidates[1].base_mass = scalar("1/3");
    document.source.source_locator = "Generic finite MaxEnt export fixture".to_owned();
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document
}

fn ten_stage_serial() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title = "Ten-stage GEN1 derivation with explicit stopping witness".to_owned();
    document.description =
        "Generic serial stress fixture; it is not attributed to a published derivation.".to_owned();
    document.evaluator = EvaluatorKind::Ot;
    document.source.constraints = vec![
        constraint("serial-c1", "MARKEDNESS", "1", 0),
        constraint("serial-c2", "FAITHFULNESS", "1", 1),
    ];
    document.source.candidates = vec![candidate(
        "serial-register",
        "registered carrier",
        "stage-0",
        vec![0, 0],
    )];
    document.source.input = "stage-0".to_owned();
    document.source.name = "SERIAL REGISTER".to_owned();
    document.source.source_locator = "Generic ten-stage serial export fixture".to_owned();
    document.target = document.source.clone();
    document.dataset = vec![document.source.clone()];
    document.serial.start = "stage-0".to_owned();
    document.serial.maximum_steps = 16;
    for stage in 0..10 {
        let from = format!("stage-{stage}");
        let to = format!("stage-{}", stage + 1);
        document.serial.moves.push(SerialMove {
            from: from.clone(),
            to: from.clone(),
            operation: "identity candidate".to_owned(),
            violations: vec![1, 0],
        });
        document.serial.moves.push(SerialMove {
            from,
            to,
            operation: if stage == 5 {
                "apply one locally bounded phonological change with a deliberately long operation label"
                    .to_owned()
            } else {
                format!("apply local change {}", stage + 1)
            },
            violations: vec![0, 1],
        });
    }
    document.serial.moves.push(SerialMove {
        from: "stage-10".to_owned(),
        to: "stage-10".to_owned(),
        operation: "identity / faithful convergence".to_owned(),
        violations: vec![0, 0],
    });
    document.serial.moves.push(SerialMove {
        from: "stage-10".to_owned(),
        to: "stage-10-alternative".to_owned(),
        operation: "nonoptimal continuation".to_owned(),
        violations: vec![1, 0],
    });
    document
}

fn serial_cycle() -> ConvalgenDocument {
    let mut document = ten_stage_serial();
    document.title = "Serial cycle refusal".to_owned();
    document.serial.start = "cycle-a".to_owned();
    document.serial.maximum_steps = 8;
    document.serial.moves = vec![
        SerialMove {
            from: "cycle-a".to_owned(),
            to: "cycle-b".to_owned(),
            operation: "change a to b".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "cycle-b".to_owned(),
            to: "cycle-a".to_owned(),
            operation: "change b to a".to_owned(),
            violations: vec![0, 0],
        },
    ];
    document
}

fn second_order_preservation() -> ConvalgenDocument {
    let mut document = reference_cases::dissertation_second_order();
    document.title = "Exact winner-set preservation under identity".to_owned();
    document.target = document.source.clone();
    document.second_order.query = QueryKind::WinnerSet;
    document.second_order.answer_sort = "set of candidate identities".to_owned();
    document.second_order.scope = "complete registered candidate support".to_owned();
    document.second_order.transformation = "identity".to_owned();
    document.second_order.transport = "identity on candidate identities".to_owned();
    document.second_order.layout = SecondOrderLayout::Overlay;
    document
}

fn second_order_refusal() -> ConvalgenDocument {
    let mut document = reference_cases::dissertation_second_order();
    document.title = "Dependency-indexed Second-Order refusal".to_owned();
    document.second_order.transport.clear();
    document.second_order.layout = SecondOrderLayout::ExpandedPaired;
    document
}

fn serial_second_order_lanes() -> ConvalgenDocument {
    let mut document = reference_cases::dissertation_second_order();
    document.title = "Equal terminal result, discrepant serial trajectory".to_owned();
    document.second_order.query = QueryKind::WinnerSet;
    document.second_order.answer_sort = "typed serial response".to_owned();
    document.second_order.scope =
        "terminal result and complete trajectory in separate lanes".to_owned();
    document.second_order.transformation = "replace the first local step".to_owned();
    document.second_order.transport = "identity on serial form identities".to_owned();
    document.second_order.layout = SecondOrderLayout::ExpandedPaired;
    document.serial.start = "a".to_owned();
    document.serial.moves = vec![
        SerialMove {
            from: "a".to_owned(),
            to: "b".to_owned(),
            operation: "source step".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "b".to_owned(),
            to: "d".to_owned(),
            operation: "source completion".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "d".to_owned(),
            to: "d".to_owned(),
            operation: "identity".to_owned(),
            violations: vec![0, 0],
        },
    ];
    document.target_serial.start = "a".to_owned();
    document.target_serial.moves = vec![
        SerialMove {
            from: "a".to_owned(),
            to: "c".to_owned(),
            operation: "target step".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "c".to_owned(),
            to: "d".to_owned(),
            operation: "target completion".to_owned(),
            violations: vec![0, 0],
        },
        SerialMove {
            from: "d".to_owned(),
            to: "d".to_owned(),
            operation: "identity".to_owned(),
            violations: vec![0, 0],
        },
    ];
    document
}

fn long_unicode_tableau() -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title =
        "Long-label and Unicode/IPA export stress fixture with grapheme-safe wrapping".to_owned();
    document.presentation.show_legend = true;
    let mut first = constraint(
        "long-c1",
        "ALIGN-EVERY-PROSODIC-WORD-WITH-THE-LEFT-EDGE-OF-ITS-LEXICAL-CORRESPONDENT-WITHOUT-TRUNCATION",
        "1",
        0,
    );
    first.definition = "Assign one violation for each prosodic word whose left edge is not aligned.\nThe complete definition is retained and wrapped rather than abbreviated.".to_owned();
    let mut second = constraint("unicode-c2", "IDENT-IO[voice, place, continuant]", "1", 1);
    second.definition = "IPA: [ɬ ʈ ɳ ɲ ʁ ɐ ỹ]; arrows: → ↓; relations: ≠ Σ ρ".to_owned();
    let long_form = format!(
        "[{}{}]",
        "a\u{0301}\u{0325}\u{0330}".repeat(20),
        "ɬʈɳɲʁɐỹ".repeat(9)
    );
    let tableau = Tableau {
        id: "visual-long-unicode".to_owned(),
        name: "LONG LABELS AND IPA".to_owned(),
        input: "/a\u{0301}\u{0325}\u{0330} → ḥ ɬ ʈ ɳ ɲ ʁ ɐ ỹ/".to_owned(),
        constraints: vec![first, second],
        candidates: vec![
            candidate(
                "unicode-a",
                "long faithful candidate",
                long_form,
                vec![0, 0],
            ),
            candidate(
                "unicode-b",
                "precomposed and decomposed IPA",
                "[ḥ a\u{0301}\u{0325}\u{0330} ɬ ʈ ɳ ɲ ʁ]",
                vec![1, 2],
            ),
            candidate("unicode-c", "right-to-left label", "[سلام → ɐ̃]", vec![2, 1]),
        ],
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: Some(EvaluatorKind::Ot),
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: vec!["long faithful candidate".to_owned()],
        source_locator: "Generic Unicode and label-length stress fixture".to_owned(),
    };
    install_tableau(document, EvaluatorKind::Ot, tableau)
}

fn dense_tableau(candidates: usize, constraints: usize) -> ConvalgenDocument {
    let mut document = ConvalgenDocument::blank();
    document.title =
        format!("Dense {candidates} × {constraints} constraint-tableau stress fixture");
    document.description =
        "Generic deterministic density fixture; it is not attributed to a published analysis."
            .to_owned();
    let constraint_register = (0..constraints)
        .map(|index| {
            constraint(
                format!("dense-c-{index}"),
                format!("C{}-DECLARED-CONSTRAINT", index + 1),
                "1",
                index,
            )
        })
        .collect::<Vec<_>>();
    let rows = (0..candidates)
        .map(|row| {
            candidate(
                format!("dense-row-{row}"),
                format!("candidate-{}", row + 1),
                format!("[dense-form-{:02}-ɐ̃]", row + 1),
                (0..constraints)
                    .map(|column| ((row * 7 + column * 3) % 4) as u16)
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let tableau = Tableau {
        id: format!("visual-dense-{candidates}-{constraints}"),
        name: format!("DENSE {candidates} × {constraints}"),
        input: "/dense/".to_owned(),
        constraints: constraint_register,
        candidates: rows,
        tie_policy: "retain all co-winners".to_owned(),
        notes: String::new(),
        evaluator: Some(EvaluatorKind::Ot),
        temperature: None,
        missing_dependencies: Vec::new(),
        expected_winners: Vec::new(),
        source_locator: "Generic dense export stress fixture".to_owned(),
    };
    install_tableau(document, EvaluatorKind::Ot, tableau)
}

fn signed_weight_plot() -> ConvalgenDocument {
    let mut document = reference_cases::pater_hg();
    document.title = "Signed constraint-weight diagnostic".to_owned();
    document.source.constraints[0].weight = Some(scalar("-3/2"));
    document.source.constraints[1].weight = Some(scalar("0"));
    document.source.constraints[2].weight = Some(scalar("2"));
    document.plot = PlotKind::ConstraintWeights;
    document
}

fn main() {
    let destination = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("ConvalGEN source crate is nested under the component root")
                .join("validation/exports")
        });
    fs::create_dir_all(&destination)
        .unwrap_or_else(|error| panic!("could not create {}: {error}", destination.display()));

    let strict = reference_cases::prince_smolensky_ot();
    write_all(
        &destination,
        "strict-ot-tableau",
        &export::tableau_svg(&strict, false).expect("strict OT visual"),
        2.0,
    );
    let ties = strict_tie_partial_order();
    write_all(
        &destination,
        "strict-ot-ties-partial-order",
        &export::tableau_svg(&ties, false).expect("strict tie visual"),
        2.0,
    );

    let hg = reference_cases::pater_hg();
    write_all(
        &destination,
        "harmonic-grammar-tableau",
        &export::tableau_svg(&hg, false).expect("HG visual"),
        2.0,
    );
    let hg_ties = exact_hg_ties();
    write_all(
        &destination,
        "harmonic-grammar-exact-ties",
        &export::tableau_svg(&hg_ties, false).expect("exact HG visual"),
        2.0,
    );

    let maxent = nonuniform_maxent();
    write_all(
        &destination,
        "maxent-tableau",
        &export::tableau_svg(&maxent, false).expect("MaxEnt visual"),
        2.0,
    );

    let serial = ten_stage_serial();
    write_all(
        &destination,
        "serial-derivation",
        &export::serial_svg(&serial).expect("serial visual"),
        1.0,
    );
    let cycle = serial_cycle();
    write_all(
        &destination,
        "serial-cycle-refusal",
        &export::serial_svg(&cycle).expect("serial cycle visual"),
        2.0,
    );

    let preserved = second_order_preservation();
    write_all(
        &destination,
        "second-order-overlay-preservation",
        &export::tableau_svg(&preserved, true).expect("Second-Order preservation visual"),
        2.0,
    );
    let mut discrepant = reference_cases::dissertation_second_order();
    discrepant.second_order.layout = SecondOrderLayout::DeltaSidecar;
    let discrepancy_svg =
        export::tableau_svg(&discrepant, true).expect("Second-Order discrepancy visual");
    write_all(
        &destination,
        "second-order-delta-discrepancy",
        &discrepancy_svg,
        2.0,
    );
    let refused = second_order_refusal();
    write_all(
        &destination,
        "second-order-paired-refusal",
        &export::tableau_svg(&refused, true).expect("Second-Order refusal visual"),
        2.0,
    );
    let serial_lanes = serial_second_order_lanes();
    write_all(
        &destination,
        "second-order-expanded-serial-lanes",
        &export::tableau_svg(&serial_lanes, true).expect("Second-Order serial lanes visual"),
        2.0,
    );

    let q = reference_cases::finnish_ranking_space();
    write_all(
        &destination,
        "q-calculus-derivation",
        &export::q_calculus_svg(&q).expect("Q visual"),
        2.0,
    );
    let mut q_refusal = q.clone();
    q_refusal.title = "Q-Calculus indexed refusal".to_owned();
    q_refusal.clone_constraint = usize::MAX;
    write_all(
        &destination,
        "q-calculus-refusal",
        &export::q_calculus_svg(&q_refusal).expect("Q refusal visual"),
        2.0,
    );

    let mut probabilities = maxent.clone();
    probabilities.plot = PlotKind::CandidateProbabilities;
    write_all(
        &destination,
        "plot-candidate-probabilities",
        &export::plot_svg(&probabilities).expect("probability plot"),
        2.0,
    );
    let mut ranking = q.clone();
    ranking.plot = PlotKind::RankingShares;
    write_all(
        &destination,
        "plot-ranking-shares",
        &export::plot_svg(&ranking).expect("ranking plot"),
        2.0,
    );
    let mut serial_plot = serial.clone();
    serial_plot.plot = PlotKind::SerialPath;
    write_all(
        &destination,
        "plot-serial-path",
        &export::plot_svg(&serial_plot).expect("serial plot"),
        2.0,
    );
    let signed = signed_weight_plot();
    write_all(
        &destination,
        "plot-signed-weights",
        &export::plot_svg(&signed).expect("signed-weight plot"),
        2.0,
    );
    let mut score_plot = hg_ties.clone();
    score_plot.plot = PlotKind::CandidateScores;
    write_all(
        &destination,
        "plot-candidate-costs",
        &export::plot_svg(&score_plot).expect("candidate-cost plot"),
        2.0,
    );

    let unicode = long_unicode_tableau();
    write_all(
        &destination,
        "long-label-unicode-tableau",
        &export::tableau_svg(&unicode, false).expect("Unicode stress visual"),
        2.0,
    );
    let dense = dense_tableau(30, 24);
    write_all(
        &destination,
        "dense-30x24-tableau",
        &export::tableau_svg(&dense, false).expect("30 by 24 stress visual"),
        1.0,
    );
    let very_dense = dense_tableau(60, 80);
    let very_dense_svg = export::tableau_svg(&very_dense, false).expect("60 by 80 stress visual");
    write_all(&destination, "dense-60x80-tableau", &very_dense_svg, 1.0);
    let publication_directory = destination.join("dense-60x80-publication-pages");
    fs::create_dir_all(&publication_directory).unwrap_or_else(|error| {
        panic!(
            "could not create {}: {error}",
            publication_directory.display()
        )
    });
    for path in export::write_publication_pdf_tiles(
        &very_dense_svg,
        &publication_directory.join("dense-60x80-a3-landscape"),
        1_191.0,
        842.0,
        36.0,
    )
    .expect("A3 landscape publication tiling")
    {
        println!("{}", path.display());
    }
}
