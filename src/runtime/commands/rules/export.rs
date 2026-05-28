//! `specrun rules export` handler — RFC-28 §"Resolved rules export".
//!
//! Read-only. Builds the `ResolveInputs` struct from CLI args,
//! delegates to [`specify_lints::build_resolved_rules`], and
//! streams the resulting envelope to stdout as JSON. v1 supports
//! JSON output only; the global `--format text` default at the
//! `Cli` level surfaces as `Error::Argument` (exit 2) so the
//! JSON-only contract is explicit.
//!
//! Failure modes round-trip through `Exit::from(&Error)` per
//! `docs/standards/handler-shape.md`:
//!
//! | `ResolveError` variant   | `Error` variant         | Exit |
//! |--------------------------|-------------------------|------|
//! | `RulesRootRequired`      | `Validation`            | 2    |
//! | `DuplicateRuleId`        | `Validation`            | 2    |
//! | `Parse`                  | `Validation`            | 2    |
//! | `Filesystem`             | `Filesystem { op }`     | 1    |

use std::path::{Path, PathBuf};

use specify_error::{Error, Result, ValidationStatus, ValidationSummary};
use specify_lints::{ResolveError, ResolveInputs, build_resolved_rules};

use crate::runtime::cli::Format;
use crate::runtime::output;

/// Run the export with explicit pre-parsed arguments. Splitting the
/// signature off the clap-struct keeps the dispatcher arm flat.
#[expect(
    clippy::too_many_arguments,
    reason = "Arguments mirror the closed RFC-28 §Resolution inputs set; the handler threads the clap-derived surface through verbatim into ResolveInputs."
)]
pub fn run(
    format: Format, rules_root: Option<&Path>, target: &str, sources: &[String],
    artifacts: &[PathBuf], languages: &[String], include_deprecated: bool, include_unmatched: bool,
    project_dir: &Path,
) -> Result<()> {
    require_json(format)?;

    let inputs = ResolveInputs {
        project_dir,
        rules_root,
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: artifacts,
        languages,
        include_deprecated,
        include_unmatched,
    };

    let resolved = build_resolved_rules(&inputs).map_err(map_resolve_error)?;

    output::emit(Box::new(std::io::stdout().lock()), Format::Json, &resolved, |_, _| Ok(()))?;
    Ok(())
}

/// Reject `--format text` with an argument-shape error (exit 2).
fn require_json(format: Format) -> Result<()> {
    match format {
        Format::Json => Ok(()),
        Format::Text => Err(Error::Argument {
            flag: "--format",
            detail: "specrun rules export currently emits JSON only; rerun with --format json"
                .to_string(),
        }),
    }
}

/// Translate the CH-12/CH-14 resolver's typed error into the CLI's
/// closed [`Error`] enum so `Exit::from(&Error)` picks the right
/// exit code per `docs/standards/handler-shape.md`.
pub fn map_resolve_error(err: ResolveError) -> Error {
    match err {
        ResolveError::RulesRootRequired => Error::Validation {
            results: vec![ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: "rules-root-required".to_string(),
                rule: "shared UNI-* rules require --rules-root or a project-local \
                       adapters/shared/rules/universal/ tree"
                    .to_string(),
                detail: Some(
                    "pass --rules-root pointing at a tree containing \
                     adapters/shared/rules/universal/"
                        .to_string(),
                ),
            }],
        },
        ResolveError::DuplicateRuleId { id, paths } => Error::Validation {
            results: vec![ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: "rules-duplicate-rule-id".to_string(),
                rule: format!("rule id '{id}' appears in multiple files"),
                detail: Some(paths),
            }],
        },
        ResolveError::Parse { path, error } => Error::Validation {
            results: vec![ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: "rules-parse-error".to_string(),
                rule: format!("failed to parse rule {}", path.display()),
                detail: Some(error.to_string()),
            }],
        },
        ResolveError::Filesystem { path, source } => Error::Filesystem {
            op: "readdir",
            path,
            source,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// `--format text` must fail with `Error::Argument` so
    /// `Exit::from` lands on exit 2 (argument shape).
    #[test]
    fn rejects_text_format_with_argument_error() {
        let err =
            run(Format::Text, None, "omnia", &[], &[], &[], false, false, &PathBuf::from("."))
                .expect_err("text format must be rejected");
        match err {
            Error::Argument { flag, detail } => {
                assert_eq!(flag, "--format");
                assert!(detail.contains("--format json"), "detail missing hint: {detail}");
            }
            other => panic!("expected Error::Argument, got {other:?}"),
        }
    }

    /// `rules-root-required` from CH-12 maps to a single-finding
    /// `Error::Validation` so the wire envelope carries the closed
    /// kebab discriminant in `results[].rule-id`.
    #[test]
    fn maps_rules_root_required_to_validation() {
        let err = map_resolve_error(ResolveError::RulesRootRequired);
        match err {
            Error::Validation { results } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].rule_id, "rules-root-required");
            }
            other => panic!("expected Error::Validation, got {other:?}"),
        }
    }

    /// `DuplicateRuleId` lands on `Error::Validation` with the
    /// colliding id in the `rule` field and the comma-joined paths
    /// in `detail`.
    #[test]
    fn maps_duplicate_rule_id_to_validation() {
        let err = map_resolve_error(ResolveError::DuplicateRuleId {
            id: "UNI-001".into(),
            paths: "a.md, b.md".into(),
        });
        match err {
            Error::Validation { results } => {
                assert_eq!(results[0].rule_id, "rules-duplicate-rule-id");
                assert!(results[0].rule.contains("UNI-001"));
                assert_eq!(results[0].detail.as_deref(), Some("a.md, b.md"));
            }
            other => panic!("expected Error::Validation, got {other:?}"),
        }
    }

    /// Filesystem failures map to `Error::Filesystem { op: "readdir" }`
    /// so the JSON discriminant becomes `filesystem-readdir` (exit 1).
    #[test]
    fn maps_filesystem_to_filesystem_error() {
        let err = map_resolve_error(ResolveError::Filesystem {
            path: PathBuf::from("/missing"),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        });
        match err {
            Error::Filesystem { op, path, .. } => {
                assert_eq!(op, "readdir");
                assert_eq!(path, PathBuf::from("/missing"));
            }
            other => panic!("expected Error::Filesystem, got {other:?}"),
        }
    }
}
