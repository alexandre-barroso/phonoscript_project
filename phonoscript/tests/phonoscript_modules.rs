use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use num_bigint::BigInt;
use num_rational::BigRational;
use phonoscript::model::ConvalgenDocument;
use phonoscript::phonoscript_frontend::Severity;
use phonoscript::phonoscript_runtime::{
    self, Number, RuntimeLimits, Value, check_file, check_file_with_limits, run_file,
    run_file_with_limits,
};

static NEXT_TREE: AtomicU64 = AtomicU64::new(1);

struct ModuleTree {
    base: PathBuf,
    root: PathBuf,
}

impl ModuleTree {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "phonoscript-module-{label}-{}-{sequence}",
            std::process::id()
        ));
        let root = base.join("root");
        fs::create_dir_all(&root).expect("create isolated module tree");
        Self { base, root }
    }

    fn write(&self, relative: &str, source: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create module parent");
        }
        fs::write(&path, source).expect("write module source");
        path
    }

    fn entry(&self) -> PathBuf {
        self.root.join("main.phont")
    }
}

impl Drop for ModuleTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn has_error(result: &[phonoscript_runtime::RuntimeDiagnostic], code: &str) -> bool {
    result
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error && diagnostic.code == code)
}

#[test]
fn selective_aliases_are_immutable_and_exported_functions_keep_private_closures() {
    let tree = ModuleTree::new("privacy");
    tree.write(
        "library.phont",
        r#"
let secret = 40
export let published = 41
export fn add_secret(n) { return secret + n }
"#,
    );
    tree.write(
        "main.phont",
        r#"
import { published as answer, add_secret as add } from "./library.phont"
assert_equal(answer, 41)
add(2)
"#,
    );
    let result = run_file(&tree.entry(), &tree.root, &ConvalgenDocument::blank());
    assert!(result.succeeded(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value,
        Value::Number(Number::Exact(BigRational::from_integer(BigInt::from(42))))
    );

    tree.write(
        "private.phont",
        "import { secret } from \"./library.phont\"\n",
    );
    let private = check_file(&tree.root.join("private.phont"), &tree.root);
    assert!(has_error(&private, "PSR0104"));
    assert!(private.iter().any(|diagnostic| {
        diagnostic.code == "PSR0104" && diagnostic.source_name == "private.phont"
    }));

    tree.write(
        "immutable.phont",
        "import { published as answer } from \"./library.phont\"\nanswer = 0\n",
    );
    let immutable = run_file(
        &tree.root.join("immutable.phont"),
        &tree.root,
        &ConvalgenDocument::blank(),
    );
    assert!(!immutable.committed);
    assert!(has_error(&immutable.diagnostics, "PSA1004"));
}

#[test]
fn canonical_duplicate_imports_load_one_declaration_environment() {
    let tree = ModuleTree::new("canonical-cache");
    tree.write(
        "library.phont",
        "export let first = 1\nexport let second = 2\n",
    );
    fs::create_dir_all(tree.root.join("sub")).expect("create canonical alias directory");
    tree.write(
        "main.phont",
        r#"
import { first } from "./library.phont"
import { second } from "./sub/../library.phont"
assert_equal(first + second, 3)
"#,
    );
    let result = run_file(&tree.entry(), &tree.root, &ConvalgenDocument::blank());
    assert!(result.succeeded(), "{:?}", result.diagnostics);
    assert_eq!(result.statistics.modules_loaded, 1);
    assert!(result.standard_output.is_empty());
}

#[test]
fn cycles_and_root_escapes_are_structured_admission_failures() {
    let tree = ModuleTree::new("cycle");
    tree.write(
        "main.phont",
        "import { b } from \"./b.phont\"\nexport let a = 1\n",
    );
    tree.write(
        "b.phont",
        "import { a } from \"./main.phont\"\nexport let b = 2\n",
    );
    let cycle = check_file(&tree.entry(), &tree.root);
    let diagnostic = cycle
        .iter()
        .find(|diagnostic| diagnostic.code == "PSR0102")
        .expect("cycle diagnostic");
    assert!(
        diagnostic
            .message
            .contains("main.phont -> b.phont -> main.phont")
    );
    assert!(!diagnostic.call_stack.is_empty());

    let parent = tree.root.parent().expect("temporary parent");
    let outside = parent.join(format!(
        "phonoscript-outside-{}-{}.phont",
        std::process::id(),
        NEXT_TREE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&outside, "export let escaped = 1\n").expect("write outside module");
    let outside_name = outside
        .file_name()
        .expect("outside filename")
        .to_string_lossy();
    tree.write(
        "escape.phont",
        &format!("import {{ escaped }} from \"../{outside_name}\"\n"),
    );
    let escaped = check_file(&tree.root.join("escape.phont"), &tree.root);
    assert!(has_error(&escaped, "PSR0101"));
    fs::remove_file(outside).expect("remove outside module");
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_after_canonicalization() {
    use std::os::unix::fs::symlink;

    let tree = ModuleTree::new("symlink");
    let outside_tree = ModuleTree::new("symlink-outside");
    let outside = outside_tree.write("outside.phont", "export let escaped = 1\n");
    symlink(&outside, tree.root.join("linked.phont")).expect("create module symlink");
    tree.write("main.phont", "import { escaped } from \"./linked.phont\"\n");
    let diagnostics = check_file(&tree.entry(), &tree.root);
    assert!(has_error(&diagnostics, "PSR0101"));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "PSR0101" && diagnostic.message.contains("outside")
    }));
}

#[test]
fn module_count_depth_and_aggregate_source_budgets_are_deterministic() {
    let tree = ModuleTree::new("budgets");
    tree.write(
        "main.phont",
        "import { answer } from \"./library.phont\"\nanswer\n",
    );
    tree.write("library.phont", "export let answer = 42\n");

    for limits in [
        RuntimeLimits {
            maximum_modules: 1,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            maximum_module_depth: 0,
            ..RuntimeLimits::default()
        },
        RuntimeLimits {
            maximum_module_source_bytes: 8,
            ..RuntimeLimits::default()
        },
    ] {
        let diagnostics = check_file_with_limits(&tree.entry(), &tree.root, limits);
        assert!(has_error(&diagnostics, "PSR0103"), "{diagnostics:?}");
    }
}

#[test]
fn nested_module_parse_and_runtime_diagnostics_retain_source_identity_and_frames() {
    let tree = ModuleTree::new("diagnostics");
    tree.write("broken.phont", "export fn broken() {\n");
    tree.write(
        "parse-main.phont",
        "import { broken } from \"./broken.phont\"\n",
    );
    let parsed = check_file(&tree.root.join("parse-main.phont"), &tree.root);
    assert!(parsed.iter().any(|diagnostic| {
        diagnostic.source_name == "broken.phont" && diagnostic.code == "PSP0106"
    }));

    tree.write(
        "failure.phont",
        r#"
fn divide(n) { return n / 0 }
export fn explode(n) { return divide(n) }
"#,
    );
    tree.write(
        "main.phont",
        "import { explode } from \"./failure.phont\"\nexplode(1)\n",
    );
    let failed = run_file(&tree.entry(), &tree.root, &ConvalgenDocument::blank());
    let diagnostic = failed
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "PSR0402")
        .expect("nested runtime diagnostic");
    assert_eq!(diagnostic.source_name, "failure.phont");
    assert_eq!(
        diagnostic
            .call_stack
            .iter()
            .map(|frame| (frame.function.as_str(), frame.source_name.as_str()))
            .collect::<Vec<_>>(),
        [("explode", "main.phont"), ("divide", "failure.phont")]
    );
}

#[test]
fn module_mutations_share_one_document_and_any_later_fault_rolls_back_everything() {
    let tree = ModuleTree::new("transaction");
    tree.write(
        "success-library.phont",
        "export fn configure() { project_title(\"module title\"); return true }\n",
    );
    tree.write(
        "main.phont",
        "import { configure } from \"./success-library.phont\"\nassert(configure())\n",
    );
    let initial = ConvalgenDocument::blank();
    let success = run_file(&tree.entry(), &tree.root, &initial);
    assert!(success.succeeded(), "{:?}", success.diagnostics);
    assert_eq!(success.document.title, "module title");

    let rollback_path = tree.base.join("must-not-exist.ottab");
    let rollback_source_path = rollback_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    tree.write(
        "failure-library.phont",
        &format!(
            r#"
export fn fail() {{
    project_title("must roll back")
    save("{rollback_source_path}")
    assert(false, "module failure")
}}
"#
        ),
    );
    tree.write(
        "failure-main.phont",
        "import { fail } from \"./failure-library.phont\"\nfail()\n",
    );
    let failed = run_file_with_limits(
        &tree.root.join("failure-main.phont"),
        &tree.root,
        &initial,
        RuntimeLimits::default(),
    );
    assert!(!failed.committed);
    assert_eq!(failed.document, initial);
    assert!(has_error(&failed.diagnostics, "PSR0450"));
    assert!(!rollback_path.exists());
}

#[test]
fn importing_a_library_never_executes_top_level_project_output_or_file_effects() {
    let tree = ModuleTree::new("declaration-only");
    let forbidden_path = tree.base.join("forbidden.ottab");
    let forbidden_source_path = forbidden_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    tree.write(
        "library.phont",
        &format!(
            "print(\"must not print\")\nproject_title(\"must not run\")\nsave(\"{forbidden_source_path}\")\nlet disguised = project_title(\"must not run either\")\nvar mutable_state = 0\nexport let answer = 42\n"
        ),
    );
    tree.write(
        "main.phont",
        "import { answer } from \"./library.phont\"\nanswer\n",
    );
    let initial = ConvalgenDocument::blank();
    let result = run_file(&tree.entry(), &tree.root, &initial);
    assert!(!result.committed);
    assert_eq!(result.document, initial);
    assert!(has_error(&result.diagnostics, "PSR0105"));
    assert!(result.standard_output.is_empty());
    assert!(!forbidden_path.exists());
}

#[test]
fn source_only_entry_points_refuse_imports_without_filesystem_authority() {
    let source = "import { answer } from \"./library.phont\"\nanswer\n";
    let checked = phonoscript_runtime::check_named("memory.phont", source);
    assert!(has_error(&checked, "PSR0101"));

    let initial = ConvalgenDocument::blank();
    let result = phonoscript_runtime::run_named("memory.phont", source, &initial);
    assert!(!result.committed);
    assert_eq!(result.document, initial);
    assert!(has_error(&result.diagnostics, "PSR0101"));
}

#[test]
fn entry_and_import_paths_must_be_phont_files_under_an_existing_root() {
    let tree = ModuleTree::new("entry-security");
    let entry = tree.write("main.txt", "1\n");
    let wrong_extension = check_file(&entry, &tree.root);
    assert!(has_error(&wrong_extension, "PSR0101"));

    let missing_root = tree.root.join("missing-root");
    let missing = check_file(Path::new(&tree.root.join("main.phont")), &missing_root);
    assert!(has_error(&missing, "PSR0101"));
}

#[test]
fn cli_module_root_executes_checks_and_maps_nested_json_sources() {
    let tree = ModuleTree::new("cli");
    tree.write("library.phont", "export let answer = 42\n");
    tree.write(
        "main.phont",
        "import { answer } from \"./library.phont\"\nanswer\n",
    );

    let checked = Command::new(env!("CARGO_BIN_EXE_phonoscript"))
        .arg("--module-root")
        .arg(&tree.root)
        .arg("--check")
        .arg("--json")
        .arg(tree.entry())
        .output()
        .expect("run module-aware CLI check");
    assert!(checked.status.success(), "{checked:?}");
    let checked_json: serde_json::Value =
        serde_json::from_slice(&checked.stdout).expect("decode check JSON");
    assert_eq!(checked_json["status"], "ok");
    assert_eq!(checked_json["language_version"], 3);

    let executed = Command::new(env!("CARGO_BIN_EXE_phonoscript"))
        .arg("--module-root")
        .arg(&tree.root)
        .arg("--json")
        .arg(tree.entry())
        .output()
        .expect("run module-aware CLI execution");
    assert!(executed.status.success(), "{executed:?}");
    let executed_json: serde_json::Value =
        serde_json::from_slice(&executed.stdout).expect("decode execution JSON");
    assert_eq!(executed_json["value"]["type"], "exact_number");
    assert_eq!(executed_json["value"]["numerator"], "42");
    assert_eq!(executed_json["statistics"]["modules_loaded"], 1);

    tree.write("failure.phont", "export fn explode() { return 1 / 0 }\n");
    tree.write(
        "main.phont",
        "import { explode } from \"./failure.phont\"\nexplode()\n",
    );
    let failed = Command::new(env!("CARGO_BIN_EXE_phonoscript"))
        .arg("--module-root")
        .arg(&tree.root)
        .arg("--json")
        .arg(tree.entry())
        .output()
        .expect("run failing module-aware CLI execution");
    assert!(!failed.status.success());
    let failed_json: serde_json::Value =
        serde_json::from_slice(&failed.stdout).expect("decode failure JSON");
    assert_eq!(failed_json["status"], "error");
    let diagnostic = failed_json["diagnostics"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["code"] == "PSR0402"))
        .expect("division-by-zero JSON diagnostic");
    assert_eq!(diagnostic["source"], "failure.phont");
    assert_eq!(diagnostic["call_stack"][0]["source"], "main.phont");
}
