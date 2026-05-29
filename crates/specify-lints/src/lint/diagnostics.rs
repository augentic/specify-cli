//! Diagnostic formatter umbrella per the diagnostic formatter contract
//! and the diagnostics formatter set.
//!
//! v1 (Phase 2) ships the four formatters the diagnostics formatter set names as the
//! closed Phase 2 set ([`Format::Json`], [`Format::Pretty`],
//! [`Format::Github`], [`Format::Compact`]). Rendering lives in this
//! module so `specrun lint` (Phase 2) and `specdev lint --format
//! json` cannot drift.
//!
//! Only the [`Format::Json`] formatter validates against
//! [`specify_schema::LINT_RESULT_JSON_SCHEMA`] before emit; the
//! other three are presentation layers driven by the same in-memory
//! [`LintResult`].

pub mod compact;
pub mod github;
pub mod json;
pub mod pretty;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use specify_error::{Error, Result};
use specify_schema::{WORKSPACE_MODEL_JSON_SCHEMA, validate_serialisable};
use thiserror::Error;

use crate::lint::WorkspaceModel;
use crate::lint::eval::HintError;
use crate::lint::index::IndexError;
use crate::rules::{FindingStatus, LintFinding, ResolvedRule, Severity};

/// Type-level pin of the [`LintResult`] envelope version.
///
/// Serialises to the integer `1` and refuses to deserialise any
/// other value for the `LintResult` envelope. Mirrors the
/// [`crate::lint::WorkspaceModelVersion`] shape so the [`Default`]
/// derive propagates through [`LintResult`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LintResultVersion;

impl Serialize for LintResultVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(1)
    }
}

impl<'de> Deserialize<'de> for LintResultVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        if value == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported LintResult version: {value} (only v1 is supported)"
            )))
        }
    }
}

/// Finding tally by severity for the `LintResult` envelope.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintSummary {
    /// Count of findings with `severity: critical`.
    pub critical: u32,
    /// Count of findings with `severity: important`.
    pub important: u32,
    /// Count of findings with `severity: suggestion`.
    pub suggestion: u32,
    /// Count of findings with `severity: optional`.
    pub optional: u32,
}

impl LintSummary {
    /// Tally `findings` by severity.
    #[must_use]
    pub fn from_findings(findings: &[LintFinding]) -> Self {
        let mut summary = Self::default();
        for finding in findings {
            match finding.severity {
                Severity::Critical => summary.critical += 1,
                Severity::Important => summary.important += 1,
                Severity::Suggestion => summary.suggestion += 1,
                Severity::Optional => summary.optional += 1,
            }
        }
        summary
    }
}

/// `LintResult` envelope review-result envelope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintResult {
    /// Envelope version discriminant pinned to `1`.
    pub version: LintResultVersion,
    /// Finding tally by severity.
    pub summary: LintSummary,
    /// Byte-stable list of structured review findings. Ordering is
    /// the producer's responsibility; this module preserves the
    /// input order on every formatter.
    pub findings: Vec<LintFinding>,
}

/// Closed the diagnostics formatter set Phase 2 formatter discriminant.
///
/// Kept clap-free at the standards-layer boundary; the
/// `specrun lint` CLI in S9 adapts this enum to its own
/// `clap::ValueEnum` so the standards crate stays runtime-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// `LintResult` wire envelope; schema-validated before emit.
    Json,
    /// Terminal output with severity colour and source location.
    Pretty,
    /// GitHub Actions workflow-annotation lines.
    Github,
    /// Tab-separated one-line-per-finding shape.
    Compact,
}

/// Closed render error per the diagnostic formatter contract.
///
/// Only the [`Format::Json`] formatter validates against
/// [`specify_schema::LINT_RESULT_JSON_SCHEMA`] before emit, so it
/// is the only formatter that can surface
/// [`RenderError::JsonSchemaValidation`]. [`RenderError::JsonSerialise`]
/// is unreachable in practice given the typed [`LintResult`] input
/// but is preserved so the JSON serialiser failure is not collapsed
/// onto a panic.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RenderError {
    /// JSON envelope failed [`specify_schema::LINT_RESULT_JSON_SCHEMA`].
    #[error("review-result envelope failed schema validation: {detail}")]
    JsonSchemaValidation {
        /// Joined `; `-separated validator error list.
        detail: String,
    },
    /// `serde_json::to_string_pretty` failed.
    #[error("review-result JSON serialisation failed: {0}")]
    JsonSerialise(#[from] serde_json::Error),
}

/// Render `result` using the requested `format`.
///
/// # Errors
///
/// - [`RenderError::JsonSchemaValidation`] when `format` is
///   [`Format::Json`] and the serialised envelope fails the v1
///   schema.
/// - [`RenderError::JsonSerialise`] when JSON serialisation itself
///   fails (unreachable for a typed [`LintResult`]).
pub fn render(format: Format, result: &LintResult) -> Result<String, RenderError> {
    match format {
        Format::Json => json::render(result),
        Format::Pretty => pretty::render(result),
        Format::Github => github::render(result),
        Format::Compact => compact::render(result),
    }
}

/// Serialise the model, validate it against the v1 schema, and print
/// it to stdout. Validation failure is an internal bug — wrapped as
/// `Error::Diag` (exit 1) per lint exit mapping.
///
/// # Errors
///
/// - `Error::Validation` when the serialised model fails the
///   [`WORKSPACE_MODEL_JSON_SCHEMA`] v1 schema.
/// - `Error::Diag { review-dump-model-serialise }` when JSON
///   serialisation itself fails.
pub fn emit_dump_model(model: &WorkspaceModel) -> Result<()> {
    validate_serialisable(
        model,
        WORKSPACE_MODEL_JSON_SCHEMA,
        "review-dump-model-schema",
        "WorkspaceModel matches workspace-model.schema.json",
        "review-dump-model-serialise",
        "WorkspaceModel",
    )?;
    let rendered = serde_json::to_string_pretty(model).map_err(|err| Error::Diag {
        code: "review-dump-model-serialise",
        detail: format!("failed to serialise WorkspaceModel: {err}"),
    })?;
    println!("{rendered}");
    Ok(())
}

/// Count findings whose `status` matches `target`.
///
/// Passing `None` counts the `open` bucket per RFC-33a — an unset
/// `status` is treated as `Open`, matching the status-aware exit
/// predicate in [`crate::lint::ignore::blocking_findings_present`].
#[must_use]
pub fn count_status(findings: &[LintFinding], target: Option<FindingStatus>) -> u32 {
    let count = findings
        .iter()
        .filter(|f| {
            target.map_or_else(
                || matches!(f.status, None | Some(FindingStatus::Open)),
                |want| f.status == Some(want),
            )
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Map a `lint::index::IndexError` onto the lint exit mapping exit-code table.
///
/// | `IndexError`                | `Error` variant                            | Exit |
/// |-----------------------------|--------------------------------------------|------|
/// | `UnsupportedScanProfile`    | `Validation { review-unsupported-scan-profile }` | 2 |
/// | `ProjectDirMissing`         | `Validation { review-project-dir-missing }`      | 2 |
/// | `OverrideCompile`           | `Validation { review-index-override-compile }`   | 2 |
/// | `Filesystem`                | `Validation { review-index-filesystem }`         | 2 |
#[must_use]
pub fn map_index_error(err: IndexError) -> Error {
    match err {
        IndexError::UnsupportedScanProfile(profile) => Error::validation_failed(
            "review-unsupported-scan-profile",
            "scan profile is not supported",
            format!("requested scan profile: {profile:?}"),
        ),
        IndexError::ProjectDirMissing(path) => Error::validation_failed(
            "review-project-dir-missing",
            "project directory does not exist",
            path.display().to_string(),
        ),
        IndexError::Filesystem(detail) => Error::validation_failed(
            "review-index-filesystem",
            "filesystem error during indexer walk",
            detail,
        ),
        IndexError::OverrideCompile(detail) => Error::validation_failed(
            "review-index-override-compile",
            "always-ignore override pattern failed to compile",
            detail,
        ),
    }
}

/// Map a `lint::eval::HintError` onto the lint exit mapping exit-code table.
///
/// | `HintError`        | `Error` variant                                  | Exit |
/// |--------------------|--------------------------------------------------|------|
/// | `Unsupported`      | `Validation { review-unsupported-hint-kind }`    | 2    |
/// | `SchemaCompile`    | `Validation { review-schema-compile-failed }`    | 2    |
/// | `SchemaResolve`    | `Validation { review-schema-resolve-failed }`    | 2    |
/// | `RegexCompile`     | `Validation { review-regex-compile-failed }`     | 2    |
/// | `ToolInvocation`   | `Validation { review-tool-invocation-failed }`   | 2    |
/// | `ToolUndeclared`   | `Validation { review-tool-undeclared }`          | 2    |
/// | `Filesystem`       | `Filesystem { op: "review-eval" }`               | 1    |
#[must_use]
pub fn map_hint_error(rule: &ResolvedRule, err: HintError) -> Error {
    match err {
        HintError::Unsupported {
            rule_id,
            kind,
            reason,
        } => Error::validation_failed(
            "review-unsupported-hint-kind",
            format!("rule {rule_id}: hint kind {kind:?} is not supported in v1"),
            reason.to_string(),
        ),
        HintError::SchemaCompile {
            rule_id,
            schema_ref,
            detail,
        } => Error::validation_failed(
            "review-schema-compile-failed",
            format!("rule {rule_id}: schema {schema_ref} failed to compile"),
            detail,
        ),
        HintError::SchemaResolve {
            rule_id,
            schema_ref,
            reason,
        } => Error::validation_failed(
            "review-schema-resolve-failed",
            format!("rule {rule_id}: schema {schema_ref} could not be resolved"),
            reason,
        ),
        HintError::RegexCompile {
            rule_id,
            pattern,
            source,
        } => Error::validation_failed(
            "review-regex-compile-failed",
            format!("rule {rule_id}: regex {pattern} failed to compile"),
            source.to_string(),
        ),
        HintError::ToolInvocation {
            rule_id,
            tool,
            detail,
        } => Error::validation_failed(
            "review-tool-invocation-failed",
            format!("rule {rule_id}: tool {tool} invocation failed"),
            detail,
        ),
        HintError::ToolUndeclared { rule_id, tool } => Error::validation_failed(
            "review-tool-undeclared",
            format!("rule {rule_id}: tool {tool} not declared by the project"),
            format!("declare {tool} in tools.yaml or remove the hint (rule path: {})", rule.path),
        ),
        HintError::Filesystem { path, source, .. } => Error::Filesystem {
            op: "review-eval",
            path,
            source,
        },
    }
}

/// Map a `lint::diagnostics::RenderError` onto the lint exit mapping exit-code table.
///
/// Both variants are internal bugs (the typed envelope cannot
/// legally fail v1 schema validation or JSON serialisation); the
/// mapping exists so the failure surface is uniform.
///
/// | `RenderError`              | `Error` variant                             | Exit |
/// |----------------------------|---------------------------------------------|------|
/// | `JsonSchemaValidation`     | `Diag { review-envelope-schema }`           | 1    |
/// | `JsonSerialise`            | `Diag { review-envelope-serialise }`        | 1    |
#[must_use]
pub fn map_render_error(err: RenderError) -> Error {
    match err {
        RenderError::JsonSchemaValidation { detail } => Error::Diag {
            code: "review-envelope-schema",
            detail,
        },
        RenderError::JsonSerialise(source) => Error::Diag {
            code: "review-envelope-serialise",
            detail: source.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{LintResultVersion, LintSummary};
    use crate::rules::{
        Artifact, Confidence, FindingEvidence, FindingSource, LintFinding, Severity,
    };

    fn finding(id: &str, severity: Severity) -> LintFinding {
        LintFinding {
            id: id.into(),
            rule_id: None,
            related_rule_ids: None,
            title: "t".into(),
            severity,
            source: FindingSource::Deterministic,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: None,
            evidence: FindingEvidence::Snippet { value: "x".into() },
            impact: "i".into(),
            remediation: "r".into(),
            confidence: Some(Confidence::High),
            fingerprint: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            status: None,
            disposition: None,
        }
    }

    #[test]
    fn version_serialises_as_one() {
        let v = serde_json::to_value(LintResultVersion).expect("serialise");
        assert_eq!(v, Value::from(1));
    }

    #[test]
    fn version_rejects_other_values() {
        let err = serde_json::from_value::<LintResultVersion>(Value::from(2))
            .expect_err("v2 must be rejected");
        assert!(err.to_string().contains("unsupported LintResult version"));
    }

    #[test]
    fn summary_counts_each_severity() {
        let findings = vec![
            finding("a", Severity::Critical),
            finding("b", Severity::Important),
            finding("c", Severity::Important),
            finding("d", Severity::Suggestion),
            finding("e", Severity::Optional),
        ];
        let summary = LintSummary::from_findings(&findings);
        assert_eq!(summary.critical, 1);
        assert_eq!(summary.important, 2);
        assert_eq!(summary.suggestion, 1);
        assert_eq!(summary.optional, 1);
    }
}
