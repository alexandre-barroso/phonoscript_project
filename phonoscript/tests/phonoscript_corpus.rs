//! Executable-corpus checks for the public PhonoScript programs.
//!
//! Every checked-in program is run from a blank document through the same
//! transactional runtime that ConvalGEN embeds.  The programs contain their
//! own result assertions; this test prevents a syntax or engine change from
//! leaving the public corpus as unexecuted documentation.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use phonoscript::{model::ConvalgenDocument, phonoscript_runtime};

fn scripts_under(root: &Path, scripts: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", root.display()))
        .map(|entry| entry.expect("directory entry is readable").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            scripts_under(&path, scripts);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(phonoscript_runtime::EXTENSION))
        {
            scripts.push(path);
        }
    }
}

#[test]
fn every_public_phonoscript_program_executes_and_checks_its_claims() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("validation/analyses");
    let mut scripts = Vec::new();
    assert!(root.is_dir(), "missing script corpus {}", root.display());
    scripts_under(&root, &mut scripts);
    scripts.sort();
    scripts.dedup();
    assert_eq!(
        scripts.len(),
        24,
        "the executable corpus must contain the 24 reviewed programs"
    );

    let mut failures = Vec::new();
    let mut names = BTreeSet::new();
    let mut sources = BTreeSet::new();
    for path in &scripts {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("program filename is UTF-8");
        assert!(names.insert(name), "duplicate program filename {name}");
        assert!(
            sources.insert(source.clone()),
            "{} duplicates another public program byte for byte",
            path.display()
        );
        assert!(
            source.starts_with("#!/usr/bin/env phonoscript\n"),
            "{} lacks the portable PhonoScript shebang",
            path.display()
        );
        assert!(
            !source.contains("project_restore_v"),
            "{} is an embedded project payload, not an authored conformance program",
            path.display()
        );
        let result = phonoscript_runtime::run(&source, &ConvalgenDocument::blank());
        if !result.succeeded() {
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
            failures.push(diagnostics);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} PhonoScript programs failed:\n{}",
        failures.len(),
        scripts.len(),
        failures.join("\n")
    );
}

#[test]
fn unix_shebang_is_valid_phonoscript_trivia() {
    let source = "#!/usr/bin/env phonoscript\nassert_equal(1/10 + 2/10, 3/10);\n";
    let result = phonoscript_runtime::run(source, &ConvalgenDocument::blank());
    assert!(result.succeeded(), "{:?}", result.diagnostics);
}
