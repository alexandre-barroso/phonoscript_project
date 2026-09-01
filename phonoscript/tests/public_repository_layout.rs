//! Public-repository layout contract.
//!
//! Project PhonoScript deliberately keeps one Markdown landing page and three
//! compiled manuals under `docs/`. This regression prevents generated notes
//! or internal agent records from silently becoming public documentation.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("phonoscript crate is inside the workspace")
        .to_path_buf()
}

fn markdown_files(root: &Path, directory: &Path, found: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("could not inspect {}: {error}", directory.display()))
        .map(|entry| entry.expect("repository entry is readable").path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            let relative = path.strip_prefix(root).expect("path remains in workspace");
            if matches!(
                relative
                    .components()
                    .next()
                    .and_then(|part| part.as_os_str().to_str()),
                Some(".git" | ".local_sources" | "target")
            ) {
                continue;
            }
            markdown_files(root, &path, found);
        } else if path.extension().is_some_and(|extension| {
            let extension = extension.to_string_lossy();
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        }) {
            found.push(path);
        }
    }
}

#[test]
fn root_readme_is_the_only_public_markdown_file() {
    let root = workspace_root();
    let mut markdown = Vec::new();
    markdown_files(&root, &root, &mut markdown);
    markdown.sort();
    assert_eq!(markdown, [root.join("README.md")]);

    let ignore = fs::read_to_string(root.join(".gitignore")).expect("workspace .gitignore");
    assert!(ignore.lines().any(|line| line.trim() == "*.md"));
    assert!(ignore.lines().any(|line| line.trim() == "!/README.md"));
}

#[test]
fn all_three_public_manuals_have_substantive_pdfs_in_docs() {
    let docs = workspace_root().join("docs");
    for stem in [
        "PhonoScript-Language-Manual",
        "ConvalGEN-User-Guide",
        "Q-Calculus-Manual",
    ] {
        let pdf = docs.join(format!("{stem}.pdf"));
        assert!(pdf.is_file(), "missing compiled manual {}", pdf.display());
        assert!(
            fs::metadata(&pdf).expect("manual metadata").len() > 1_000,
            "{} is not a substantive PDF",
            pdf.display()
        );
    }
    assert!(!docs.join("README.md").exists());

    let ignore =
        fs::read_to_string(workspace_root().join(".gitignore")).expect("workspace .gitignore");
    assert!(ignore.lines().any(|line| line.trim() == "*.[tT][eE][xX]"));
    assert!(
        ignore
            .lines()
            .any(|line| line.trim() == "!/docs/*.[pP][dD][fF]")
    );
}
