//! GitHub Actions workflow-annotation formatter per RFC-32 §D6.
//!
//! One `::<level> file=…,line=…,col=…,title=…::<message>` line per
//! finding. Escaping follows the GitHub workflow-command rules
//! (<https://docs.github.com/actions/using-workflows/workflow-commands-for-github-actions#example-of-a-workflow-command>):
//!
//! - `%` -> `%25`, `\r` -> `%0D`, `\n` -> `%0A` everywhere.
//! - Inside an argument list (between `::` and `::`) `,` -> `%2C`
//!   and `:` -> `%3A` so the argument separator round-trips.
//!
//! The post-`::` message body is not argument-parsed; only the three
//! universal escapes apply.

use std::fmt::Write as _;

use super::{RenderError, ReviewResult};
use crate::codex::{ReviewFinding, Severity};

/// Render `result` as one GitHub workflow-annotation line per
/// finding.
///
/// # Errors
///
/// Never errors — the [`Result`] return mirrors the uniform
/// [`super::render`] dispatch signature.
pub fn render(result: &ReviewResult) -> Result<String, RenderError> {
    let mut out = String::new();
    for finding in &result.findings {
        write_finding(&mut out, finding);
    }
    Ok(out)
}

fn write_finding(out: &mut String, finding: &ReviewFinding) {
    let level = github_level(finding.severity);
    let mut args: Vec<String> = Vec::new();
    if let Some(loc) = finding.location.as_ref() {
        args.push(format!("file={}", escape_arg(&loc.path)));
        if let Some(line) = loc.line {
            args.push(format!("line={line}"));
        }
        if let Some(col) = loc.column {
            args.push(format!("col={col}"));
        }
    }
    args.push(format!("title={}", escape_arg(&finding.title)));
    let arg_list = args.join(",");
    let rule_tag = finding.rule_id.as_deref().map_or(String::new(), |id| format!(" [{id}]"));
    let body = format!(
        "{title}{rule_tag}\n  Impact: {impact}\n  Remediation: {remediation}",
        title = finding.title,
        impact = finding.impact,
        remediation = finding.remediation
    );
    let _ = writeln!(out, "::{level} {arg_list}::{}", escape_body(&body));
}

const fn github_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical | Severity::Important => "error",
        Severity::Suggestion => "warning",
        Severity::Optional => "notice",
    }
}

fn escape_arg(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            ',' => out.push_str("%2C"),
            ':' => out.push_str("%3A"),
            other => out.push(other),
        }
    }
    out
}

fn escape_body(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            other => out.push(other),
        }
    }
    out
}
