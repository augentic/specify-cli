//! `validate_slice` — the top-level runner that walks the canonical
//! refine-time artifact set, locates each artifact, invokes the
//! registered rules, and collects a [`ValidationReport`].
//!
//! workflow §"Refinement" pins the canonical artifact set to
//! `proposal.md`, `spec.md`, `design.md`, `tasks.md`, plus the
//! `contracts/` overlay; per-define-brief `generates` paths from the
//! pre-2.0 `pipeline.define[]` are gone with the legacy adapter
//! shape. Rules are still registered in
//! [`crate::registry::rules_for`] under the historical
//! per-brief namespaces (`proposal`, `specs`, `design`, `tasks`,
//! `contracts`); the runner just feeds artifacts into that registry
//! directly instead of routing via a `PipelineView`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use specify_error::{Error, ValidationStatus, ValidationSummary};

use crate::registry::{cross_rules, rules_for};
use crate::{BriefContext, Classification, CrossContext, RuleOutcome, ValidationReport};

const DEFERRED_REASON: &str = "Semantic check — requires LLM judgment";

/// Canonical refine-time artifact set, in registry-namespace order.
///
/// `(brief_id, artifact)` where `artifact` is either a literal path
/// relative to the slice dir or a glob (containing `*`). Mirrors the
/// validation registry's namespaces verbatim — rules are registered
/// under these ids in [`crate::registry::rules_for`].
const CANONICAL_ARTIFACTS: &[(&str, &str)] = &[
    ("proposal", "proposal.md"),
    ("specs", "specs/**/*.md"),
    ("design", "design.md"),
    ("tasks", "tasks.md"),
    ("contracts", "contracts/**/*.yaml"),
];

fn summary(
    status: ValidationStatus, rule_id: impl Into<String>, rule: impl Into<String>,
    detail: Option<String>,
) -> ValidationSummary {
    ValidationSummary {
        status,
        rule_id: rule_id.into(),
        rule: rule.into(),
        detail,
    }
}

/// Run all deterministic validations for a slice directory.
///
/// Iterates the canonical refine-time artifact set
/// (`CANONICAL_ARTIFACTS`). Plain entries are stat-checked once;
/// glob entries are expanded via the `glob` crate and only existing
/// matches are walked. Empty glob results are silently skipped — an
/// absent `specs/login/spec.md` is not, by itself, a failure.
///
/// # Errors
///
/// Returns an error if a glob pattern is malformed or a glob traversal
/// fails.
pub fn validate_slice(slice_dir: &Path) -> Result<ValidationReport, Error> {
    let mut brief_results: BTreeMap<String, Vec<ValidationSummary>> = BTreeMap::new();
    let specs_dir = slice_dir.join("specs");

    for (brief_id, artifact) in CANONICAL_ARTIFACTS {
        let artifacts = expand_artifact(slice_dir, artifact)?;

        if artifacts.is_empty() {
            // Glob matched nothing. If the configured path is literal,
            // treat that as "artifact missing" so the skill sees the
            // failure. Globs that legitimately match nothing are
            // skipped silently — Specify 2.0 slices don't have to populate
            // every overlay (e.g. `contracts/`).
            if !artifact.contains('*') {
                let missing_path = slice_dir.join(artifact);
                let key = (*brief_id).to_string();
                let results = vec![artifact_missing_result(brief_id, &missing_path, slice_dir)];
                brief_results.insert(key, results);
            }
            continue;
        }

        let single_artifact = artifacts.len() == 1 && !artifact.contains('*');
        for artifact_path in artifacts {
            let key = if single_artifact {
                (*brief_id).to_string()
            } else {
                relative_key(slice_dir, &artifact_path)
            };

            let results = run_brief_rules(brief_id, &artifact_path, slice_dir, &specs_dir);
            brief_results.insert(key, results);
        }
    }

    let cross_checks = run_cross_rules(slice_dir, &specs_dir);

    let passed = brief_results
        .values()
        .flatten()
        .chain(cross_checks.iter())
        .all(|r| r.status != ValidationStatus::Fail);

    Ok(ValidationReport {
        brief_results,
        cross_checks,
        passed,
    })
}

/// Expand `artifact` into a concrete list of absolute paths under
/// `slice_dir`. Plain paths are returned as a singleton (regardless of
/// existence — the runner checks that separately). Glob patterns
/// (containing `*`) are expanded via the `glob` crate and only existing
/// matches are returned.
fn expand_artifact(slice_dir: &Path, artifact: &str) -> Result<Vec<PathBuf>, Error> {
    let joined = slice_dir.join(artifact);
    if !artifact.contains('*') {
        return Ok(vec![joined]);
    }
    let pattern = joined.to_str().ok_or_else(|| Error::Diag {
        code: "validate-glob-non-utf8",
        detail: format!("non-UTF8 glob pattern `{}`", joined.display()),
    })?;
    let mut out: Vec<PathBuf> = Vec::new();
    let entries = glob::glob(pattern).map_err(|err| Error::Diag {
        code: "validate-glob-invalid",
        detail: format!("invalid glob `{pattern}`: {err}"),
    })?;
    for entry in entries {
        match entry {
            Ok(path) if path.is_file() => out.push(path),
            Ok(_) => {}
            Err(err) => {
                return Err(Error::Diag {
                    code: "validate-glob-traversal-failed",
                    detail: format!("glob traversal failure: {err}"),
                });
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Build the key used to index `ValidationReport.brief_results` for
/// multi-artifact briefs. We strip `slice_dir` to make the key stable
/// across different tempdir prefixes; unix-style forward slashes are used
/// so golden fixtures compare identically across platforms.
fn relative_key(slice_dir: &Path, artifact_path: &Path) -> String {
    let rel = artifact_path.strip_prefix(slice_dir).unwrap_or(artifact_path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn artifact_missing_result(
    brief_id: &str, artifact_path: &Path, slice_dir: &Path,
) -> ValidationSummary {
    let rel = relative_key(slice_dir, artifact_path);
    summary(
        ValidationStatus::Fail,
        format!("{brief_id}.artifact-exists"),
        format!("Generated artifact {rel} exists"),
        Some(format!("artifact `{rel}` not found under slice dir")),
    )
}

fn run_brief_rules(
    brief_id: &str, artifact_path: &Path, slice_dir: &Path, specs_dir: &Path,
) -> Vec<ValidationSummary> {
    let Ok(content) = std::fs::read_to_string(artifact_path) else {
        return vec![artifact_missing_result(brief_id, artifact_path, slice_dir)];
    };

    // Parse brief-specific structured context.
    let parsed_spec = (brief_id == "specs").then(|| specify_model::spec::parse_baseline(&content));
    let tasks = (brief_id == "tasks").then(|| specify_model::task::parse_tasks(&content));

    let ctx = BriefContext {
        id: brief_id,
        content: &content,
        parsed_spec: parsed_spec.as_ref(),
        tasks: tasks.as_ref(),
        slice_dir,
        specs_dir,
    };

    let mut out: Vec<ValidationSummary> = Vec::new();
    for rule in rules_for(brief_id) {
        let result = rule.check.map_or_else(
            || {
                summary(
                    ValidationStatus::Deferred,
                    rule.id,
                    rule.description,
                    Some(DEFERRED_REASON.into()),
                )
            },
            |check| match check(&ctx) {
                RuleOutcome::Pass => {
                    summary(ValidationStatus::Pass, rule.id, rule.description, None)
                }
                RuleOutcome::Fail { detail } => {
                    summary(ValidationStatus::Fail, rule.id, rule.description, Some(detail))
                }
            },
        );
        out.push(result);
    }
    out
}

fn run_cross_rules(slice_dir: &Path, specs_dir: &Path) -> Vec<ValidationSummary> {
    let ctx = CrossContext { slice_dir, specs_dir };
    let mut out: Vec<ValidationSummary> = Vec::new();
    for rule in cross_rules() {
        let result = match rule.classification {
            Classification::Semantic => summary(
                ValidationStatus::Deferred,
                rule.id,
                rule.description,
                Some(DEFERRED_REASON.into()),
            ),
            Classification::Structural => match (rule.check)(&ctx) {
                RuleOutcome::Pass => {
                    summary(ValidationStatus::Pass, rule.id, rule.description, None)
                }
                RuleOutcome::Fail { detail } => {
                    summary(ValidationStatus::Fail, rule.id, rule.description, Some(detail))
                }
            },
        };
        out.push(result);
    }
    out
}
