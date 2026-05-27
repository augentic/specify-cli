//! Terminal formatter per RFC-32 §D6 — severity tag, rule id, title,
//! source location, and an indented impact/remediation block per
//! finding plus a tally footer mirroring [`super::ReviewSummary`].
//!
//! ANSI escape codes are hand-written so the crate avoids a colour
//! dependency. The `NO_COLOR` environment variable, when set
//! (regardless of value, per <https://no-color.org>), suppresses
//! the escape codes so tests and pipelines get plain text.

use std::fmt::Write as _;

use super::{RenderError, ReviewResult};
use crate::rules::{FindingLocation, ReviewFinding, Severity};

/// Render `result` as colourised terminal output.
///
/// # Errors
///
/// Never errors — the [`Result`] return mirrors the uniform
/// [`super::render`] dispatch signature.
pub fn render(result: &ReviewResult) -> Result<String, RenderError> {
    let mut out = String::new();
    let _ = writeln!(out, "Specify review — {} finding(s)", result.findings.len());
    for finding in &result.findings {
        write_finding(&mut out, finding);
    }
    let s = &result.summary;
    let _ = writeln!(
        out,
        "Summary: {} critical, {} important, {} suggestion, {} optional",
        s.critical, s.important, s.suggestion, s.optional
    );
    Ok(out)
}

fn write_finding(out: &mut String, finding: &ReviewFinding) {
    let tag = paint(finding.severity, severity_tag(finding.severity));
    let rule = finding.rule_id.as_deref().map_or(String::new(), |id| format!(" {id}"));
    let location = finding
        .location
        .as_ref()
        .map_or(String::new(), |loc| format!(" ({})", format_location(loc)));
    let _ = writeln!(out, "{tag}{rule} {title}{location}", title = finding.title);
    let _ = writeln!(out, "  impact: {}", finding.impact);
    let _ = writeln!(out, "  remediation: {}", finding.remediation);
}

const fn severity_tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "[CRITICAL]",
        Severity::Important => "[IMPORTANT]",
        Severity::Suggestion => "[SUGGESTION]",
        Severity::Optional => "[OPTIONAL]",
    }
}

fn paint(severity: Severity, text: &str) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
        return text.to_owned();
    }
    let code = match severity {
        Severity::Critical => 31,
        Severity::Important => 33,
        Severity::Suggestion => 36,
        Severity::Optional => 34,
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn format_location(loc: &FindingLocation) -> String {
    match (loc.line, loc.column) {
        (Some(line), Some(col)) => format!("{}:{line}:{col}", loc.path),
        (Some(line), None) => format!("{}:{line}", loc.path),
        _ => loc.path.clone(),
    }
}
