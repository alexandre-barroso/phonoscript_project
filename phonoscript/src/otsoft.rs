//! Compatibility with the tab-delimited tableau format shared by OTSoft and
//! MaxEnt Grammar Tool. The import is isolated from the native `.ottab` model.

use crate::exact::NumericScalar;
use crate::model::{Candidate, Constraint, MAX_VIOLATION, Tableau};

pub fn import_tsv(text: &str) -> Result<Vec<Tableau>, String> {
    let rows: Vec<Vec<&str>> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim_end_matches('\r').split('\t').collect())
        .collect();
    if rows.len() < 2 {
        return Err("the tab-delimited file has no tableau rows".to_owned());
    }
    let header_index = rows
        .iter()
        .position(|row| row.len() > 3 && row.iter().skip(3).any(|cell| !cell.trim().is_empty()))
        .ok_or_else(|| "could not locate the constraint header".to_owned())?;
    let full_names = &rows[header_index];
    let abbreviations = rows.get(header_index + 1);
    let constraint_count = full_names.len().saturating_sub(3);
    if constraint_count == 0 {
        return Err("the file declares no constraints".to_owned());
    }
    let names: Vec<String> = (0..constraint_count)
        .map(|index| {
            let full = full_names.get(index + 3).copied().unwrap_or("").trim();
            let short = abbreviations
                .and_then(|row| row.get(index + 3))
                .copied()
                .unwrap_or("")
                .trim();
            if short.is_empty() {
                full.to_owned()
            } else {
                short.to_owned()
            }
        })
        .collect();
    if names.iter().any(String::is_empty) {
        return Err("a constraint header is blank".to_owned());
    }
    let constraints: Vec<Constraint> = names
        .iter()
        .enumerate()
        .map(|(index, name)| Constraint {
            id: format!("constraint-{}", index + 1),
            name: name.clone(),
            weight: Some(NumericScalar::integer(1)),
            stratum: index,
            enabled: true,
            definition: String::new(),
            prior_mean: NumericScalar::integer(0),
            prior_sigma: NumericScalar::integer(100_000),
        })
        .collect();
    let mut tableaus: Vec<Tableau> = Vec::new();
    let mut current_input = String::new();
    for row in rows.iter().skip(header_index + 1) {
        if row.len() < 3 {
            continue;
        }
        let candidate_name = row[1].trim();
        if candidate_name.is_empty() {
            continue;
        }
        if !row[0].trim().is_empty() {
            current_input = row[0].trim().to_owned();
        }
        if current_input.is_empty() {
            return Err("a candidate row occurs before the first input".to_owned());
        }
        let required_cells = 3 + constraint_count;
        if row.len() < required_cells {
            return Err(format!(
                "candidate {candidate_name} has {} violation columns; expected {constraint_count}",
                row.len().saturating_sub(3)
            ));
        }
        let observed_frequency = if row[2].trim().is_empty() {
            NumericScalar::integer(0)
        } else {
            NumericScalar::parse_exact(row[2].trim())
                .map_err(|_| format!("invalid exact frequency for {candidate_name}"))?
        };
        if !matches!(observed_frequency.to_f64_center(), Ok(frequency) if frequency >= 0.0) {
            return Err(format!("invalid frequency for {candidate_name}"));
        }
        let violations: Vec<u16> = (0..constraint_count)
            .map(|index| {
                let cell = row[index + 3].trim();
                if cell.is_empty() {
                    Ok(0)
                } else {
                    let mark = cell.parse::<u16>().map_err(|_| {
                        format!("invalid violation mark {cell:?} for {candidate_name}")
                    })?;
                    if mark > MAX_VIOLATION {
                        Err(format!(
                            "violation mark {cell:?} for {candidate_name} exceeds {MAX_VIOLATION}"
                        ))
                    } else {
                        Ok(mark)
                    }
                }
            })
            .collect::<Result<_, _>>()?;
        if tableaus
            .last()
            .is_none_or(|tableau| tableau.input != current_input)
        {
            tableaus.push(Tableau {
                id: format!("tableau-{}", tableaus.len() + 1),
                name: current_input.clone(),
                input: current_input.clone(),
                constraints: constraints.clone(),
                candidates: Vec::new(),
                tie_policy: "retain all co-winners".to_owned(),
                notes: String::new(),
                evaluator: None,
                temperature: None,
                missing_dependencies: Vec::new(),
                expected_winners: Vec::new(),
                source_locator: String::new(),
            });
        }
        let candidate_id = format!(
            "candidate-{}",
            tableaus
                .last()
                .map_or(1, |tableau| tableau.candidates.len() + 1)
        );
        tableaus
            .last_mut()
            .expect("just created or previously present")
            .candidates
            .push(Candidate {
                id: candidate_id,
                name: candidate_name.to_owned(),
                form: candidate_name.to_owned(),
                violations,
                base_mass: NumericScalar::integer(1),
                notes: String::new(),
                observed_frequency,
                structured: None,
            });
    }
    if tableaus.is_empty() {
        return Err("the file contains no readable candidate rows".to_owned());
    }
    Ok(tableaus)
}

pub fn export_tsv(tableaus: &[Tableau]) -> Result<String, String> {
    let first = tableaus
        .first()
        .ok_or_else(|| "there are no tableaux to export".to_owned())?;
    if tableaus.iter().any(|tableau| {
        tableau.constraints.len() != first.constraints.len()
            || tableau
                .constraints
                .iter()
                .zip(&first.constraints)
                .any(|(left, right)| left.name != right.name)
    }) {
        return Err("OTSoft export requires one shared ordered constraint register".to_owned());
    }
    if tableaus.iter().any(|tableau| {
        tableau.candidates.iter().any(|candidate| {
            candidate
                .violations
                .iter()
                .any(|mark| *mark > MAX_VIOLATION)
        })
    }) {
        return Err(
            "OTSoft export requires every violation count to be supplied by the phonologist"
                .to_owned(),
        );
    }
    let mut output = String::from("\t\t\t");
    output.push_str(
        &first
            .constraints
            .iter()
            .map(|constraint| constraint.name.as_str())
            .collect::<Vec<_>>()
            .join("\t"),
    );
    output.push('\n');
    output.push_str("\t\t\t");
    output.push_str(
        &first
            .constraints
            .iter()
            .map(|constraint| constraint.name.as_str())
            .collect::<Vec<_>>()
            .join("\t"),
    );
    output.push('\n');
    for tableau in tableaus {
        for (index, candidate) in tableau.candidates.iter().enumerate() {
            if index == 0 {
                output.push_str(&tableau.input);
            }
            output.push('\t');
            output.push_str(&candidate.name);
            output.push('\t');
            output.push_str(&candidate.observed_frequency.canonical());
            for mark in &candidate.violations {
                output.push('\t');
                if *mark != 0 {
                    output.push_str(&mark.to_string());
                }
            }
            output.push('\n');
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::UNSET_VIOLATION;
    use crate::reference_cases;

    #[test]
    fn shared_otsoft_format_round_trips_tableaux() {
        let original = reference_cases::finite_maxent_smoke().dataset;
        let text = export_tsv(&original).expect("exports");
        let restored = import_tsv(&text).expect("imports");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].input, original[0].input);
        assert_eq!(restored[0].candidates.len(), original[0].candidates.len());
        assert_eq!(
            restored[0].candidates[0].observed_frequency,
            original[0].candidates[0].observed_frequency
        );
    }

    #[test]
    fn blank_otsoft_frequencies_mean_zero_instead_of_dropping_candidates() {
        let text = "\t\t\tFaith\tMark\n\t\t\tFA\tMA\n/input/\twinner\t10\t\t1\n\tloser\t\t1\t\n";
        let tableaus = import_tsv(text).expect("imports a blank zero frequency");
        assert_eq!(tableaus.len(), 1);
        assert_eq!(tableaus[0].candidates.len(), 2);
        assert_eq!(
            tableaus[0].candidates[1].observed_frequency,
            NumericScalar::integer(0)
        );
    }

    #[test]
    fn missing_trailing_violation_columns_are_not_silently_read_as_zero() {
        let text = "\t\t\tFaith\tMark\n\t\t\tFA\tMA\n/input/\tcandidate\t1\t0\n";
        let problem = import_tsv(text).expect_err("one required column is absent");
        assert!(problem.contains("has 1 violation columns; expected 2"));
    }

    #[test]
    fn unset_native_cells_are_not_exported_as_otsoft_counts() {
        let mut tableaus = reference_cases::finite_maxent_smoke().dataset;
        tableaus[0].candidates[0].violations[0] = UNSET_VIOLATION;
        let problem = export_tsv(&tableaus).expect_err("unset cells must block export");
        assert!(problem.contains("supplied by the phonologist"));
    }
}
