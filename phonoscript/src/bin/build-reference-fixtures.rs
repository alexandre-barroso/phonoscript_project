use std::fs;
use std::path::PathBuf;

use phonoscript::{document, reference_cases};

fn main() {
    let destination = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join("reference")
        });
    fs::create_dir_all(&destination)
        .unwrap_or_else(|error| panic!("could not create {}: {error}", destination.display()));

    let cases = [
        (
            "prince-smolensky-ot",
            reference_cases::prince_smolensky_ot(),
        ),
        ("pater-hg", reference_cases::pater_hg()),
        (
            "finite-maxent-smoke",
            reference_cases::finite_maxent_smoke(),
        ),
        ("tessier-hg-maxent", reference_cases::tessier_hg_maxent()),
        (
            "goldwater-johnson-finnish-ledger",
            reference_cases::goldwater_johnson_finnish_ledger(),
        ),
        (
            "serial-syllabification-smoke",
            reference_cases::serial_syllabification_smoke(),
        ),
        (
            "dissertation-second-order",
            reference_cases::dissertation_second_order(),
        ),
        (
            "finnish-ranking-space",
            reference_cases::finnish_ranking_space(),
        ),
        (
            "dissertation-complete",
            reference_cases::dissertation_project(),
        ),
    ];

    for (name, project) in cases {
        let requested = destination.join(format!("{name}.ottab"));
        let path = document::save(&requested, &project)
            .unwrap_or_else(|error| panic!("could not build {}: {error}", requested.display()));
        println!("{}", path.display());
    }
}
