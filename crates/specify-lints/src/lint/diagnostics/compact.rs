//! Tab-separated one-finding-per-line formatter per RFC-32 §D6 —
//! grep- and PR-bot-friendly.
//!
//! Line shape:
//!
//! ```text
//! <severity>\t<rule-id|->\t<path|->:<line|->:<col|->\t<title>
//! ```
//!
//! Tab over comma so that finding titles, rule ids, and remediation
//! strings (which often contain commas and rarely contain tabs) do
//! not need escaping.

use std::fmt::Write as _;

use super::{LintResult, RenderError};
use crate::rules::{LintFinding, Severity};

/// Render `result` as one tab-separated line per finding plus a
/// trailing newline.
///
/// # Errors
///
/// Never errors — the [`Result`] return mirrors the uniform
/// [`super::render`] dispatch signature.
pub fn render(result: &LintResult) -> Result<String, RenderError> {
    let mut out = String::new();
    for finding in &result.findings {
        write_finding(&mut out, finding);
    }
    Ok(out)
}

fn write_finding(out: &mut String, finding: &LintFinding) {
    let severity = severity_token(finding.severity);
    let rule_id = finding.rule_id.as_deref().unwrap_or("-");
    let (path, line, col) = finding.location.as_ref().map_or_else(
        || ("-", "-".to_owned(), "-".to_owned()),
        |loc| {
            (
                loc.path.as_str(),
                loc.line.map_or_else(|| "-".to_owned(), |n| n.to_string()),
                loc.column.map_or_else(|| "-".to_owned(), |n| n.to_string()),
            )
        },
    );
    let _ =
        writeln!(out, "{severity}\t{rule_id}\t{path}:{line}:{col}\t{title}", title = finding.title);
}

const fn severity_token(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "critical",
        Severity::Important => "important",
        Severity::Suggestion => "suggestion",
        Severity::Optional => "optional",
    }
}
