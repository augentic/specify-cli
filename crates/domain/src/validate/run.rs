//! `validate_slice` — the top-level runner that walks a `PipelineView`,
//! locates each brief's artifacts, invokes the registered rules, and
//! collects a [`ValidationReport`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::capability::{PipelineView, ValidationResult};
use specify_error::Error;

use crate::validate::registry::{cross_rules, rules_for};
use crate::validate::{BriefContext, Classification, CrossContext, RuleOutcome, ValidationReport};

/// Run all deterministic validations for a slice directory.
///
/// Discovers artifacts via the `generates` path on each brief's
/// frontmatter (expanding `*` via the `glob` crate when present). Briefs
/// without a `generates` field are skipped because they have no artifact
/// to inspect — only define-phase briefs produce validate-able outputs.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn validate_slice(
    slice_dir: &Path, pipeline: &PipelineView,
) -> Result<ValidationReport, Error> {
    let mut brief_results: BTreeMap<String, Vec<ValidationResult>> = BTreeMap::new();
    let specs_dir = slice_dir.join("specs");
    let terminology = infer_terminology(pipeline);

    for (_phase, brief) in &pipeline.briefs {
        let Some(generates) = brief.frontmatter.generates.as_deref() else {
            continue;
        };

        let artifacts = expand_generates(slice_dir, generates)?;
        let brief_id = brief.frontmatter.id.clone();

        if artifacts.is_empty() {
            // Glob matched nothing. If the configured `generates` is a
            // literal path, treat that as "artifact missing" so the
            // skill sees the failure. For globs that legitimately match
            // nothing (e.g. an empty `specs/**/*.md`), do the same —
            // no artifact means there is nothing to rule against.
            let missing_path = slice_dir.join(generates);
            let key = brief_id.clone();
            let results = vec![artifact_missing_result(&brief_id, &missing_path, slice_dir)];
            brief_results.insert(key, results);
            continue;
        }

        let single_artifact = artifacts.len() == 1;
        for artifact_path in artifacts {
            let key = if single_artifact {
                brief_id.clone()
            } else {
                relative_key(slice_dir, &artifact_path)
            };

            let results =
                run_brief_rules(&brief_id, &artifact_path, slice_dir, &specs_dir, terminology);
            brief_results.insert(key, results);
        }
    }

    let cross_checks = run_cross_rules(slice_dir, &specs_dir, pipeline, terminology);

    let passed = brief_results
        .values()
        .flatten()
        .chain(cross_checks.iter())
        .all(|r| !matches!(r, ValidationResult::Fail { .. }));

    Ok(ValidationReport {
        brief_results,
        cross_checks,
        passed,
    })
}

/// Infer whether to use "crate" or "feature" terminology from the capability
/// name. `vectis` uses "feature"; everything else defaults to "crate".
/// See `DECISIONS.md` §"Change G — Terminology inference" for the
/// rationale.
fn infer_terminology(pipeline: &PipelineView) -> &'static str {
    match pipeline.capability.manifest.name.as_str() {
        "vectis" => "feature",
        _ => "crate",
    }
}

/// Expand `generates` into a concrete list of absolute paths under
/// `slice_dir`. Plain paths are returned as a singleton (regardless of
/// existence — the runner checks that separately). Glob patterns
/// (containing `*`) are expanded via the `glob` crate and only existing
/// matches are returned.
fn expand_generates(slice_dir: &Path, generates: &str) -> Result<Vec<PathBuf>, Error> {
    let joined = slice_dir.join(generates);
    if !generates.contains('*') {
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
) -> ValidationResult {
    let rel = relative_key(slice_dir, artifact_path);
    ValidationResult::Fail {
        rule_id: format!("{brief_id}.artifact-exists").into(),
        rule: format!("Generated artifact {rel} exists").into(),
        detail: format!("artifact `{rel}` not found under slice dir"),
    }
}

fn run_brief_rules(
    brief_id: &str, artifact_path: &Path, slice_dir: &Path, specs_dir: &Path,
    terminology: &'static str,
) -> Vec<ValidationResult> {
    let Ok(content) = std::fs::read_to_string(artifact_path) else {
        return vec![artifact_missing_result(brief_id, artifact_path, slice_dir)];
    };

    // Parse brief-specific structured context.
    let parsed_spec = (brief_id == "specs").then(|| crate::spec::parse_baseline(&content));
    let tasks = (brief_id == "tasks").then(|| crate::task::parse_tasks(&content));

    let ctx = BriefContext {
        id: brief_id,
        content: &content,
        parsed_spec: parsed_spec.as_ref(),
        tasks: tasks.as_ref(),
        slice_dir,
        specs_dir,
        terminology,
    };

    let mut out: Vec<ValidationResult> = Vec::new();
    for rule in rules_for(brief_id) {
        let result = rule.check.map_or_else(
            || ValidationResult::Deferred {
                rule_id: rule.id.into(),
                rule: rule.description.into(),
                reason: "Semantic check — requires LLM judgment",
            },
            |check| match check(&ctx) {
                RuleOutcome::Pass => ValidationResult::Pass {
                    rule_id: rule.id.into(),
                    rule: rule.description.into(),
                },
                RuleOutcome::Fail { detail } => ValidationResult::Fail {
                    rule_id: rule.id.into(),
                    rule: rule.description.into(),
                    detail,
                },
            },
        );
        out.push(result);
    }
    out
}

fn run_cross_rules(
    slice_dir: &Path, specs_dir: &Path, pipeline: &PipelineView, terminology: &'static str,
) -> Vec<ValidationResult> {
    let ctx = CrossContext {
        slice_dir,
        specs_dir,
        pipeline,
        terminology,
    };
    let mut out: Vec<ValidationResult> = Vec::new();
    for rule in cross_rules() {
        let result = match rule.classification {
            Classification::Semantic => ValidationResult::Deferred {
                rule_id: rule.id.into(),
                rule: rule.description.into(),
                reason: "Semantic check — requires LLM judgment",
            },
            Classification::Structural => match (rule.check)(&ctx) {
                RuleOutcome::Pass => ValidationResult::Pass {
                    rule_id: rule.id.into(),
                    rule: rule.description.into(),
                },
                RuleOutcome::Fail { detail } => ValidationResult::Fail {
                    rule_id: rule.id.into(),
                    rule: rule.description.into(),
                    detail,
                },
            },
        };
        out.push(result);
    }
    out
}
