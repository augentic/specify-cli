//! Target build request/report wire DTOs + the success-blocking gate
//! (RFC-29d M3 / D6).
//!
//! Both envelopes are closed-shape and schema-validated by
//! [`crate::schema::validate_build_request_json`] /
//! [`crate::schema::validate_build_report_json`] before the verb
//! deserialises here. The request omits `target`, `execution`, brief
//! paths, and `model.yaml` (RFC-29d §"Build request"); target-specific
//! input growth is the explicit [`BuildArtifacts::additional`] list.
//! [`enforce_report_no_blocking_on_success`] is the typed gate the verb
//! applies to a deserialised report.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use specify_diagnostics::{Diagnostic, blocking};
use specify_error::{Error, Result};

/// Wire version pinned by both build schemas (`version` `const: 1`).
pub const BUILD_VERSION: u32 = 1;

/// The per-slice build request handed to a target adapter.
///
/// Round-trips `schemas/target/build-request.schema.json`. `project_dir`
/// (the working tree) and [`BuildInputs::root`] (the slice tree) are
/// distinct by design; all [`BuildArtifacts`] paths resolve against
/// `root`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildRequest {
    /// Wire version; always [`BUILD_VERSION`] per the schema `const`.
    pub version: u32,
    /// Slice being built (kebab-case).
    pub slice: String,
    /// Working tree the target builds into and validates against.
    pub project_dir: PathBuf,
    /// Slice tree plus the resolved artifact paths.
    pub inputs: BuildInputs,
}

/// The slice tree root plus the rendered artifacts the target consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildInputs {
    /// Slice tree that every [`BuildArtifacts`] path resolves against.
    pub root: PathBuf,
    /// The rendered artifact paths, relative to [`BuildInputs::root`].
    pub artifacts: BuildArtifacts,
}

/// The rendered artifact paths under [`BuildInputs::artifacts`], each
/// relative to [`BuildInputs::root`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildArtifacts {
    /// Singular rendered `proposal.md`.
    pub proposal: String,
    /// Singular rendered `design.md`.
    pub design: String,
    /// Singular rendered `tasks.md`.
    pub tasks: String,
    /// One or more per-unit `spec.md` files (`specs/<unit>/spec.md`).
    pub specs: Vec<String>,
    /// Target-specific inputs declared by the bound adapter's manifest.
    /// Empty when the adapter declares none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional: Vec<String>,
}

/// Closed build outcome enum.
///
/// Partial success is [`BuildStatus::Success`] carrying non-blocking
/// findings only — the CLI rejects a `success` report with any blocking
/// finding via [`enforce_report_no_blocking_on_success`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildStatus {
    /// Build succeeded; only non-blocking findings (or none) allowed.
    Success,
    /// Build failed; blocking findings allowed.
    Failure,
}

/// The per-slice build report a target adapter returns.
///
/// Round-trips `schemas/target/build-report.schema.json`. `findings`
/// elements are [`Diagnostic`]s governed by `diagnostic.schema.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildReport {
    /// Wire version; always [`BUILD_VERSION`] per the schema `const`.
    pub version: u32,
    /// Slice that was built; must match the request.
    pub slice: String,
    /// Adapter that produced the report (e.g. `omnia@v1`).
    pub target: String,
    /// `success` or `failure`.
    pub status: BuildStatus,
    /// RFC-28 diagnostics; defaults to `[]`.
    #[serde(default)]
    pub findings: Vec<Diagnostic>,
}

/// Reject a [`BuildStatus::Success`] report carrying any blocking
/// finding (RFC-29d §"Build report").
///
/// A finding blocks per the RFC-28 [`blocking`] predicate (an open
/// `critical` / `important` violation). On [`BuildStatus::Failure`]
/// blocking findings are allowed, so the gate is a no-op.
///
/// # Errors
///
/// Returns [`Error::Validation`] keyed on
/// `target-build-success-with-blocking-finding` (exit code 2) when a
/// `success` report carries a blocking finding.
pub fn enforce_report_no_blocking_on_success(report: &BuildReport) -> Result<()> {
    if report.status == BuildStatus::Success && report.findings.iter().any(blocking) {
        return Err(Error::validation_failed(
            "target-build-success-with-blocking-finding",
            "a success build report carries no blocking finding",
            format!("slice `{}` reported success with a blocking finding", report.slice),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A minimal schema-valid [`Diagnostic`] JSON of the given severity,
    /// left at the default `violation` kind and untriaged status so
    /// `critical` / `important` instances block.
    fn finding(severity: &str) -> Value {
        json!({
            "id": "DIAG-0001",
            "title": "test finding",
            "severity": severity,
            "source": "tool",
            "artifact": "code",
            "evidence": { "kind": "snippet", "value": "x" },
            "impact": "impact",
            "remediation": "fix it",
            "fingerprint": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        })
    }

    fn report(status: &str, findings: &[Value]) -> BuildReport {
        serde_json::from_value(json!({
            "version": 1,
            "slice": "identity-service",
            "target": "omnia@v1",
            "status": status,
            "findings": findings,
        }))
        .expect("report deserialises")
    }

    #[test]
    fn request_round_trips() {
        let req = json!({
            "version": 1,
            "slice": "identity-service",
            "project-dir": "/w/.specify/workspace/identity-service",
            "inputs": {
                "root": "/w/.specify/slices/identity-service",
                "artifacts": {
                    "proposal": "proposal.md",
                    "design": "design.md",
                    "tasks": "tasks.md",
                    "specs": ["specs/identity/spec.md"],
                    "additional": ["tokens.yaml"]
                }
            }
        });
        let parsed: BuildRequest = serde_json::from_value(req).expect("request deserialises");
        assert_eq!(parsed.version, BUILD_VERSION);
        assert_eq!(parsed.slice, "identity-service");
        assert_eq!(parsed.inputs.artifacts.specs, vec!["specs/identity/spec.md".to_string()]);
        assert_eq!(parsed.inputs.artifacts.additional, vec!["tokens.yaml".to_string()]);

        let serialised = serde_json::to_string(&parsed).expect("serialise request");
        assert!(serialised.contains("project-dir"), "project-dir renders kebab-case");
        let reparsed: BuildRequest = serde_json::from_str(&serialised).expect("re-deserialise");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn report_rejects_unknown_field() {
        let bogus = json!({
            "version": 1,
            "slice": "identity-service",
            "target": "omnia@v1",
            "status": "success",
            "findings": [],
            "stray": true
        });
        serde_json::from_value::<BuildReport>(bogus)
            .expect_err("deny_unknown_fields rejects stray keys");
    }

    #[test]
    fn gate_rejects_success_with_blocking_finding() {
        let report = report("success", &[finding("critical")]);
        match enforce_report_no_blocking_on_success(&report) {
            Err(Error::Validation { code, .. }) => {
                assert_eq!(code, "target-build-success-with-blocking-finding");
            }
            other => panic!("expected blocking-finding gate to fire, got {other:?}"),
        }
    }

    #[test]
    fn gate_accepts_success_with_only_non_blocking_findings() {
        let report = report("success", &[finding("suggestion")]);
        enforce_report_no_blocking_on_success(&report).expect("non-blocking success passes");
    }

    #[test]
    fn gate_accepts_failure_with_blocking_finding() {
        let report = report("failure", &[finding("critical")]);
        enforce_report_no_blocking_on_success(&report)
            .expect("failure may carry blocking findings");
    }
}
