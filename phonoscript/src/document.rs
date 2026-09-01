use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::ConvalgenDocument;

pub const EXTENSION: &str = "ottab";

pub fn ensure_extension(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION))
    {
        path.to_owned()
    } else {
        path.with_extension(EXTENSION)
    }
}

pub fn encode(document: &ConvalgenDocument) -> Result<Vec<u8>, String> {
    document.validate()?;
    let mut normalized = document.clone();
    normalized.normalize();
    normalized.validate()?;
    let mut bytes = serde_json::to_vec_pretty(&normalized).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<ConvalgenDocument, String> {
    let mut document: ConvalgenDocument = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid .ottab document: {error}"))?;
    document.validate()?;
    document.normalize();
    Ok(document)
}

pub fn load(path: &Path) -> Result<ConvalgenDocument, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    decode(&bytes)
}

/// Save atomically in the destination directory so an interrupted write does
/// not replace a valid analysis with a partial one.
pub fn save(path: &Path, document: &ConvalgenDocument) -> Result<PathBuf, String> {
    let destination = ensure_extension(path);
    let bytes = encode(document)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    let temporary = destination.with_extension("ottab.tmp");
    {
        let mut file = fs::File::create(&temporary)
            .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, &destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("could not replace {}: {error}", destination.display())
    })?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phonology::{SegmentTemplate, StructuredCandidate, UnderlyingForm};
    use crate::reference_cases;

    #[test]
    fn ottab_round_trip_is_self_contained_and_versioned() {
        let original = reference_cases::dissertation_second_order();
        let bytes = encode(&original).expect("encodes");
        let text = String::from_utf8(bytes.clone()).expect("utf8");
        assert!(text.contains("\"format\": \"convalgen-analysis\""));
        assert!(!text.contains("file_path"));
        let restored = decode(&bytes).expect("decodes");
        assert_eq!(restored, original);
    }

    #[test]
    fn ottab_round_trip_preserves_optional_structured_candidate_data() {
        let mut original = reference_cases::prince_smolensky_ot();
        let input = UnderlyingForm::from_segments(
            "pa",
            [SegmentTemplate::new("p"), SegmentTemplate::new("a")],
        );
        original.source.candidates[0].structured = Some(StructuredCandidate::identity(&input));
        let restored = decode(&encode(&original).expect("encodes")).expect("decodes");
        assert_eq!(
            restored.source.candidates[0].structured,
            original.source.candidates[0].structured
        );
    }

    #[test]
    fn retired_calculated_mark_declarations_are_refused_on_load() {
        let mut legacy = reference_cases::prince_smolensky_ot();
        legacy.source.constraints[0].definition = "calc: count b".to_owned();
        let bytes = serde_json::to_vec_pretty(&legacy).expect("legacy JSON serializes");
        let problem = decode(&bytes).expect_err("legacy calculated marks require manual repair");
        assert!(problem.contains("retired calculated-mark declaration"));
        assert!(problem.contains("enter every violation count explicitly"));
    }

    #[test]
    fn extension_is_enforced_without_double_suffixing() {
        assert_eq!(
            ensure_extension(Path::new("study")),
            PathBuf::from("study.ottab")
        );
        assert_eq!(
            ensure_extension(Path::new("study.OTTAB")),
            PathBuf::from("study.OTTAB")
        );
    }

    #[test]
    fn published_reference_documents_load_as_current_ottab_files() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/reference");
        let mut files: Vec<PathBuf> = fs::read_dir(directory)
            .expect("reference fixture directory")
            .map(|entry| entry.expect("directory entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == EXTENSION)
            })
            .collect();
        files.sort();
        assert!(files.len() >= 8);
        for path in files {
            let document = load(&path).expect("fixture loads");
            assert_eq!(document.format, crate::model::DOCUMENT_FORMAT);
            assert_eq!(document.format_version, crate::model::DOCUMENT_VERSION);
        }
    }
}
