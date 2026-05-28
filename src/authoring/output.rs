use std::io::Write;
use std::path::Path;

use serde::Serialize;
use specify_authoring::error::ToolingError;
use specify_authoring::finding::{Finding, Location};
use specify_error::ValidationSummary;
use specify_lints::{LintFinding, Severity};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct CheckBody {
    pub status: CheckStatus,
    pub results: Vec<ValidationSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl From<&[Finding]> for CheckBody {
    fn from(findings: &[Finding]) -> Self {
        Self {
            status: if findings.is_empty() { CheckStatus::Pass } else { CheckStatus::Fail },
            results: findings.iter().map(Finding::to_summary).collect(),
            error: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Error,
}

pub fn check_body(result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>) -> CheckBody {
    match result {
        Ok((_, findings)) => CheckBody::from(findings.as_slice()),
        Err(error) => CheckBody {
            status: CheckStatus::Error,
            results: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

pub fn write_check_text(
    w: &mut dyn Write, _body: &CheckBody,
    result: &Result<(std::path::PathBuf, Vec<Finding>), ToolingError>,
) -> std::io::Result<()> {
    match result {
        Ok((framework_root, findings)) if findings.is_empty() => writeln!(w, "All checks passed."),
        Ok((framework_root, findings)) => {
            for finding in findings {
                eprintln!("FAIL: {}: {}", finding.rule_id, finding.message);
                if let Some(location) = &finding.location {
                    eprintln!("  at {}", format_location(framework_root, location));
                }
            }
            eprintln!("{} check failure(s).", findings.len());
            Ok(())
        }
        Err(error) => {
            eprintln!("error: {error}");
            Ok(())
        }
    }
}

/// `LintResult` envelope" body emitted by
/// `specdev check --format json`.
///
/// The envelope is intentionally closed: only `version`, `summary`,
/// and `findings` ship to stdout. Infrastructure errors that prevent
/// the checks from running surface as exit code `1` plus an
/// `error: ...` line on stderr (handled by the
/// [`crate::authoring::commands::check`] handler), and the envelope on
/// stdout collapses to `{version: 1, summary: {all zero}, findings: []}`
/// so consumers can rely on a stable shape regardless of outcome.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct LintFindingsBody {
    pub version: u32,
    pub summary: SeveritySummary,
    pub findings: Vec<LintFinding>,
}

/// Per-severity counts for [`LintFindingsBody::summary`]. All four
/// keys are always present; severities with zero findings serialize as
/// `0` rather than being omitted.
#[derive(Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct SeveritySummary {
    pub critical: usize,
    pub important: usize,
    pub suggestion: usize,
    pub optional: usize,
}

/// Build a [`LintFindingsBody`] for `findings`, computing the
/// per-severity summary from the input vector.
#[must_use]
pub fn review_findings_body(findings: Vec<LintFinding>) -> LintFindingsBody {
    let mut summary = SeveritySummary::default();
    for finding in &findings {
        match finding.severity {
            Severity::Critical => summary.critical += 1,
            Severity::Important => summary.important += 1,
            Severity::Suggestion => summary.suggestion += 1,
            Severity::Optional => summary.optional += 1,
        }
    }
    LintFindingsBody {
        version: 1,
        summary,
        findings,
    }
}

fn format_location(framework_root: &Path, location: &Location) -> String {
    let path = location
        .path
        .strip_prefix(framework_root)
        .unwrap_or(&location.path)
        .display()
        .to_string()
        .replace('\\', "/");

    location.column.map_or_else(
        || format!("{path}:{}", location.line),
        |column| format!("{path}:{}:{column}", location.line),
    )
}
