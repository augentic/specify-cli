//! GitHub Actions workflow-annotation formatter.
//!
//! One `::<level> file=…,line=…,col=…,title=…::<message>` line per
//! diagnostic. Escaping follows the GitHub workflow-command rules:
//!
//! - `%` -> `%25`, `\r` -> `%0D`, `\n` -> `%0A` everywhere.
//! - Inside an argument list (between `::` and `::`) `,` -> `%2C`
//!   and `:` -> `%3A` so the argument separator round-trips.
//!
//! The post-`::` message body is not argument-parsed; only the three
//! universal escapes apply.

use std::fmt::Write as _;

use super::RenderError;
use crate::diagnostic::{Diagnostic, DiagnosticReport, Severity};

/// Render `report` as one GitHub workflow-annotation line per
/// diagnostic.
///
/// # Errors
///
/// Never errors — the [`Result`] return mirrors the uniform
/// [`super::render`] dispatch signature.
pub fn render(report: &DiagnosticReport) -> Result<String, RenderError> {
    let mut out = String::new();
    for finding in &report.findings {
        write_finding(&mut out, finding);
    }
    Ok(out)
}

fn write_finding(out: &mut String, finding: &Diagnostic) {
    let level = github_level(finding.severity);
    let mut args: Vec<String> = Vec::new();
    if let Some(loc) = finding.location.as_ref() {
        args.push(format!("file={}", escape(&loc.path, true)));
        if let Some(line) = loc.line {
            args.push(format!("line={line}"));
        }
        if let Some(col) = loc.column {
            args.push(format!("col={col}"));
        }
    }
    args.push(format!("title={}", escape(&finding.title, true)));
    let arg_list = args.join(",");
    let rule_tag = finding.rule_id.as_deref().map_or(String::new(), |id| format!(" [{id}]"));
    let body = format!(
        "{title}{rule_tag}\n  Impact: {impact}\n  Remediation: {remediation}",
        title = finding.title,
        impact = finding.impact,
        remediation = finding.remediation
    );
    let _ = writeln!(out, "::{level} {arg_list}::{}", escape(&body, false));
}

const fn github_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical | Severity::Important => "error",
        Severity::Suggestion => "warning",
        Severity::Optional => "notice",
    }
}

fn escape(s: &str, in_arg: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\r' => out.push_str("%0D"),
            '\n' => out.push_str("%0A"),
            ',' if in_arg => out.push_str("%2C"),
            ':' if in_arg => out.push_str("%3A"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape;
    use crate::diagnostic::{DiagnosticReport, DiagnosticSummary, Severity};
    use crate::render::{Format, render};
    use crate::test_support::sample_diagnostic;

    fn report(findings: Vec<crate::diagnostic::Diagnostic>) -> DiagnosticReport {
        DiagnosticReport {
            version: crate::diagnostic::DiagnosticReportVersion,
            summary: DiagnosticSummary::from_diagnostics(&findings),
            findings,
        }
    }

    #[test]
    fn annotation_carries_level_file_line_col_and_message() {
        let out = render(Format::Github, &report(vec![sample_diagnostic()])).expect("renders");
        assert!(out.starts_with("::error "), "important maps to error level, got {out:?}");
        assert!(out.contains("file=crates/invoice_export/src/config.rs"));
        assert!(out.contains("line=18"));
        assert!(out.contains("col=5"));
        assert!(
            out.contains("%0A  Impact: "),
            "body newline is escaped, colon is not, got {out:?}"
        );
    }

    #[test]
    fn severity_maps_to_github_level() {
        let mut critical = sample_diagnostic();
        critical.severity = Severity::Critical;
        let mut suggestion = sample_diagnostic();
        suggestion.severity = Severity::Suggestion;
        let mut optional = sample_diagnostic();
        optional.severity = Severity::Optional;
        let out =
            render(Format::Github, &report(vec![critical, suggestion, optional])).expect("renders");
        assert!(out.contains("::error "), "critical -> error");
        assert!(out.contains("::warning "), "suggestion -> warning");
        assert!(out.contains("::notice "), "optional -> notice");
    }

    #[test]
    fn escape_encodes_universal_and_arg_only_characters() {
        assert_eq!(escape("a%b\rc\nd", false), "a%25b%0Dc%0Ad");
        assert_eq!(escape("a,b:c", true), "a%2Cb%3Ac", "arg context escapes comma and colon");
        assert_eq!(escape("a,b:c", false), "a,b:c", "message body leaves comma and colon");
    }
}
