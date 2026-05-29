//! Shared fixture for the diagnostic formatter tests.
//!
//! Builds a [`DiagnosticReport`] exercising every shape that varies the
//! per-formatter rendering: three different severities, all three
//! [`FindingEvidence`] variants, the `Option<rule_id>` arm via one
//! anonymous finding, and varied [`FindingLocation`] coverage
//! (`line + column`, `line only`, and `no location at all`).

use serde_json::json;
use specify_lints::lint::diagnostics::{
    DiagnosticReport, DiagnosticReportVersion, DiagnosticSummary,
};
use specify_lints::rules::{
    Artifact, Confidence, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence,
    FindingLocation, Severity,
};

/// Three-finding fixture covering the rendering matrix S8 needs to
/// exercise.
pub fn make_fixture() -> DiagnosticReport {
    let findings = vec![
        Diagnostic {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-014".into()),
            related_rule_ids: None,
            title: "Literal deployment URL in generated handler".into(),
            severity: Severity::Critical,
            source: DiagnosticSource::Deterministic,
            kind: DiagnosticKind::Violation,
            target_adapter: Some("omnia".into()),
            source_adapter: None,
            slice: Some("billing-invoice-export".into()),
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: "crates/invoice_export/src/config.rs".into(),
                line: Some(18),
                column: Some(5),
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: "const BASE_URL: &str = \"https://api.example.com\";".into(),
            },
            impact: "Generated code points every deployment at one endpoint.".into(),
            remediation: "Route the endpoint through Omnia configuration.".into(),
            confidence: Some(Confidence::High),
            fingerprint: format!("sha256:{}", "11".repeat(32)),
            status: None,
            disposition: None,
        },
        Diagnostic {
            id: "FIND-0002".into(),
            rule_id: None,
            related_rule_ids: None,
            title: "Bundle digest, with comma, exceeds policy".into(),
            severity: Severity::Important,
            source: DiagnosticSource::Deterministic,
            kind: DiagnosticKind::Violation,
            target_adapter: Some("omnia".into()),
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Tests,
            location: Some(FindingLocation {
                path: "tests/fixtures/blob.bin".into(),
                line: Some(42),
                column: None,
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Digest {
                sha256: "22".repeat(32),
                summary: "binary fixture digest".into(),
                locations: None,
            },
            impact: "Cached digest drift breaks downstream reproducers.".into(),
            remediation: "Regenerate the fixture via scripts/regen-wasm-fixtures.sh.".into(),
            confidence: Some(Confidence::Medium),
            fingerprint: format!("sha256:{}", "22".repeat(32)),
            status: None,
            disposition: None,
        },
        Diagnostic {
            id: "FIND-0003".into(),
            rule_id: Some("ORG-001".into()),
            related_rule_ids: None,
            title: "Optional housekeeping note".into(),
            severity: Severity::Optional,
            source: DiagnosticSource::Deterministic,
            kind: DiagnosticKind::Violation,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Specs,
            location: None,
            evidence: FindingEvidence::Structured {
                summary: "doc coverage delta".into(),
                data: json!({ "covered": 12, "total": 13 }),
                locations: None,
            },
            impact: "One doc section lags behind the rest of the slice.".into(),
            remediation: "Backfill the missing doc section in the next refine.".into(),
            confidence: Some(Confidence::Low),
            fingerprint: format!("sha256:{}", "33".repeat(32)),
            status: None,
            disposition: None,
        },
    ];

    DiagnosticReport {
        version: DiagnosticReportVersion,
        summary: DiagnosticSummary::from_diagnostics(&findings),
        findings,
    }
}
