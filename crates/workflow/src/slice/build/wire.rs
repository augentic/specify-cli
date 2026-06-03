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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_diagnostics::{Diagnostic, blocking};
use specify_error::{Error, Result};

use crate::platform::Platform;

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

/// A single per-platform build output declared in a [`BuildReport`].
///
/// Each entry names the platform and a path (relative to `project-dir`)
/// where the target adapter produced an artifact. The CLI finalize gate
/// verifies every declared path exists and is non-empty
/// (`target-build-output-missing`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct BuildOutput {
    /// Platform this output was produced for.
    pub platform: Platform,
    /// Relative path (from `project-dir`) to the produced artifact.
    pub path: String,
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
    /// Per-platform build outputs; defaults to `[]` for backward
    /// compatibility. When non-empty the finalize gate verifies every
    /// path exists on disk.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<BuildOutput>,
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

/// Reject a [`BuildStatus::Success`] report whose `outputs[]` paths do
/// not all resolve to existing, non-empty files under `project_dir`.
///
/// Empty `outputs` is accepted (backward compatibility — the field is
/// optional). On [`BuildStatus::Failure`] the gate is a no-op (a failed
/// build need not have produced outputs).
///
/// # Errors
///
/// Returns [`Error::Validation`] keyed on
/// `target-build-output-missing` (exit code 2) when a success report
/// declares an output path that is absent, empty, not a regular file,
/// or escapes the project directory.
pub fn enforce_report_outputs_exist(report: &BuildReport, project_dir: &Path) -> Result<()> {
    if report.status != BuildStatus::Success || report.outputs.is_empty() {
        return Ok(());
    }
    for output in &report.outputs {
        let path = Path::new(&output.path);
        if path.is_absolute() || path.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(Error::validation_failed(
                "target-build-output-missing",
                "every build output path is a relative path within the project",
                format!(
                    "output for platform `{}` at `{}` is absolute or contains `..`",
                    output.platform, output.path
                ),
            ));
        }
        let full = project_dir.join(path);
        match std::fs::metadata(&full) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => {}
            Ok(meta) if !meta.is_file() => {
                return Err(Error::validation_failed(
                    "target-build-output-missing",
                    "every build output path is a regular file",
                    format!(
                        "output for platform `{}` at `{}` exists but is not a regular file",
                        output.platform, output.path
                    ),
                ));
            }
            Ok(_) => {
                return Err(Error::validation_failed(
                    "target-build-output-missing",
                    "every build output path exists and is non-empty",
                    format!(
                        "output for platform `{}` at `{}` exists but is empty",
                        output.platform, output.path
                    ),
                ));
            }
            Err(_) => {
                return Err(Error::validation_failed(
                    "target-build-output-missing",
                    "every build output path exists and is non-empty",
                    format!(
                        "output for platform `{}` at `{}` does not exist under {}",
                        output.platform,
                        output.path,
                        project_dir.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
