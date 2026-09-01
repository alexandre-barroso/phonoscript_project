use std::fs;
use std::path::{Path, PathBuf};

use phonoscript::document;
use phonoscript::model::{ConvalgenDocument, Tableau};
use phonoscript::phonological_engine::PhonologicalEngine;

fn reference_projects() -> Vec<PathBuf> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference");
    let mut paths = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", directory.display()))
        .map(|entry| entry.expect("reference directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "ottab")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

const EXPECTED_REFERENCE_PROJECTS: [&str; 9] = [
    "dissertation-complete.ottab",
    "dissertation-second-order.ottab",
    "finite-maxent-smoke.ottab",
    "finnish-ranking-space.ottab",
    "goldwater-johnson-finnish-ledger.ottab",
    "pater-hg.ottab",
    "prince-smolensky-ot.ottab",
    "serial-syllabification-smoke.ottab",
    "tessier-hg-maxent.ottab",
];

fn check_tableau(
    engine: &PhonologicalEngine,
    document: &ConvalgenDocument,
    tableau: &Tableau,
    coordinate: &str,
) {
    let result = engine.evaluate_in_project(document, tableau);
    if tableau.missing_dependencies.is_empty() {
        let result = result.unwrap_or_else(|error| panic!("{coordinate} was refused: {error}"));
        if !tableau.expected_winners.is_empty() {
            let observed = result
                .winner_indices
                .iter()
                .map(|index| tableau.candidates[*index].name.clone())
                .collect::<Vec<_>>();
            assert_eq!(
                observed, tableau.expected_winners,
                "{coordinate} did not reproduce its stored oracle"
            );
        }
    } else {
        match result {
            Err(error) => assert!(
                !error.code.is_empty() && !error.coordinate.is_empty(),
                "{coordinate} refusal was not structured"
            ),
            Ok(_) => panic!("{coordinate} evaluated despite its declared missing dependencies"),
        }
    }
}

#[test]
fn every_reference_project_runs_through_the_checked_engine() {
    let paths = reference_projects();
    let names = paths
        .iter()
        .map(|path| {
            path.file_name()
                .expect("reference project has a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, EXPECTED_REFERENCE_PROJECTS);
    let engine = PhonologicalEngine::new();
    for path in paths {
        let document = document::load(&path)
            .unwrap_or_else(|error| panic!("{} did not load: {error}", path.display()));
        let stem = path
            .file_stem()
            .expect("reference project has a stem")
            .to_string_lossy();
        check_tableau(
            &engine,
            &document,
            &document.source,
            &format!("{stem}.source"),
        );
        check_tableau(
            &engine,
            &document,
            &document.target,
            &format!("{stem}.target"),
        );
        for (index, tableau) in document.dataset.iter().enumerate() {
            check_tableau(
                &engine,
                &document,
                tableau,
                &format!("{stem}.dataset[{index}]"),
            );
        }
        if !document.serial.moves.is_empty() {
            engine
                .serial(
                    &document.source,
                    &document.serial,
                    document.source.evaluator_or(document.evaluator),
                    document.source.temperature_or(&document.temperature),
                )
                .unwrap_or_else(|error| panic!("{stem}.serial was refused: {error}"));
        }
    }
}
