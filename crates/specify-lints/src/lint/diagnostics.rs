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
use thiserror::Error;

use crate::rules::{LintFinding, Severity};

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
