//! `kind: regex` evaluator per the executable hint-kind contract.
//!
//! Compiles the hint's `value` once and walks each text candidate
//! line by line, emitting one [`crate::rules::Diagnostic`] per
//! match with a 1-indexed `line` / `column` location and the matched
//! line clipped to a bounded char count in the `Snippet` evidence
//! payload.
//!
//! Binary files are skipped (`WorkspaceModel` file scan says regex hints "skip
//! binary files unless `applicability.binary: true`" — the
//! reserved-binary flag is rejected as `unsupported` until
//! reserved-kinds policy lands, so v1 always skips). Each candidate
//! file is re-read from disk; v1 does not carry file bytes through
//! [`crate::lint::WorkspaceModel`]. A future optimisation could
//! either persist bytes on the model or share a precomputed regex
//! index — neither is required for Phase 2 correctness.

use std::path::{Path, PathBuf};

use ::regex::Regex;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::{FileKind, WorkspaceModel};
use crate::rules::{DeterministicHint, ResolvedRule};

const SNIPPET_MAX_CHARS: usize = 240;

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], project_dir: &Path,
    model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let pattern = Regex::new(&hint.value).map_err(|err| HintError::RegexCompile {
        rule_id: rule.rule_id.clone(),
        pattern: hint.value.clone(),
        source: err,
    })?;

    let mut out: Vec<Diagnostic> = Vec::new();
    for candidate in candidates {
        let candidate_str = candidate.to_string_lossy();
        if !is_text_file(model, &candidate_str) {
            continue;
        }
        let absolute = project_dir.join(candidate);
        let bytes = match std::fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(HintError::Filesystem {
                    op: "read",
                    path: absolute,
                    source: err,
                });
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        for (line_idx, line) in text.lines().enumerate() {
            for capture in pattern.find_iter(line) {
                let line_no = u32::try_from(line_idx + 1).unwrap_or(u32::MAX);
                let column_no = u32::try_from(capture.start() + 1).unwrap_or(u32::MAX);
                let location = FindingLocation {
                    path: candidate_str.to_string(),
                    line: Some(line_no),
                    column: Some(column_no),
                    end_line: None,
                    end_column: None,
                };
                let evidence = FindingEvidence::Snippet {
                    value: clip_snippet(line),
                };
                let title = format!(
                    "{}: matched `{}`",
                    rule.title,
                    hint.description.as_deref().unwrap_or(&hint.value)
                );
                let finding = make_finding(rule, *next_id, title, Some(location), evidence);
                *next_id += 1;
                out.push(finding);
            }
        }
    }
    Ok(out)
}

fn is_text_file(model: &WorkspaceModel, candidate: &str) -> bool {
    model
        .files
        .iter()
        .find(|f| f.path == candidate)
        .is_none_or(|f| matches!(f.kind, FileKind::Text))
}

fn clip_snippet(line: &str) -> String {
    if line.chars().count() <= SNIPPET_MAX_CHARS {
        return line.to_owned();
    }
    let mut out = String::new();
    for ch in line.chars().take(SNIPPET_MAX_CHARS) {
        out.push(ch);
    }
    out.push('…');
    out
}
