//! `kind: tool` evaluator per `kind: tool` evaluator contract.
//!
//! Hint `value` is the declared tool name. The runner trait the
//! evaluator is plumbed with — [`ToolRunner`] — defers WASI host
//! wiring to the CLI layer so the standards crate stays free of a
//! `wasmtime` / `specify-tool` dependency. The CLI implementation
//! lives in `specrun lint` (S9); this module only consumes the
//! abstract trait surface.
//!
//! v1 runs the tool once per candidate file and passes the
//! candidate's project-relative path as the sole positional argument.
//! The closed `{artifact}` / `{project_dir}` / `{rule_id}` placeholder
//! set named in the contract cannot be expanded in v1 because the
//! closed [`crate::rules::DeterministicHint`] shape carries no
//! `args:` field; extending the hint shape is the rules schema's responsibility,
//! not this evaluator's.
//!
//! Per `kind: tool` evaluator contract:
//!
//! - Tools the project did not declare emit a single
//!   `tool.undeclared` finding (severity `important`).
//! - Successful runs whose stdout is the `DiagnosticReport`
//!   envelope OR a single `Diagnostic` body fold the tool's
//!   findings straight into the scan result; the umbrella
//!   re-stamps `id` and `fingerprint` after applying the §"Evidence
//!   cap" truncation.
//! - Non-zero exit with no findings emits one
//!   `tool.invocation-failed` finding with the rule's severity and
//!   the (truncated) stderr in `Snippet` evidence.
//! - Runner-level invocation failures (e.g. WASI host could not
//!   start the tool) propagate as
//!   [`super::HintError::ToolInvocation`] for the caller to map to
//!   the lint exit mapping exit-code table.

use std::path::{Path, PathBuf};

use specify_diagnostics::{
    Diagnostic, DiagnosticReport, FindingEvidence, FindingLocation, Severity,
};
use thiserror::Error;

use super::{HintError, SyntheticFinding, make_synthetic_finding, restamp_finding};
use crate::rules::{DeterministicHint, ResolvedRule};

const STDERR_MAX_BYTES: usize = 8 * 1024;

/// Trait the umbrella plumbs into [`super::evaluate`] so the WASI
/// runtime stays out of the standards crate's dep graph.
///
/// `specrun lint` (S9) supplies a `wasmtime`-backed implementation;
/// integration tests in this crate supply a fake.
pub trait ToolRunner {
    /// Invoke the named tool with `args` against `project_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`ToolRunError`] when the runtime cannot deliver the
    /// tool's stdout / stderr / exit-code triple (the tool itself
    /// could not be started, the WASI sandbox refused the
    /// permission set, etc.). A successful invocation that simply
    /// exited non-zero is reported as `Ok(ToolOutput { exit_code, … })`.
    fn run(
        &self, tool_name: &str, args: &[String], project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError>;
    /// Return `true` when `tool_name` is declared by the project's
    /// `tools.yaml` (or an adapter-declared tool the project has
    /// granted `review` capability).
    fn is_declared(&self, tool_name: &str) -> bool;
}

/// Captured stdout / stderr / exit code from one tool invocation.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Verbatim stdout bytes.
    pub stdout: Vec<u8>,
    /// Verbatim stderr bytes.
    pub stderr: Vec<u8>,
    /// Process exit code (0 on success).
    pub exit_code: i32,
}

/// Closed runtime failure mode for [`ToolRunner::run`].
#[derive(Debug, Error)]
pub enum ToolRunError {
    /// Runtime could not deliver the tool's stdout / stderr / exit
    /// triple.
    #[error("tool runtime failure: {0}")]
    Runtime(String),
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], project_dir: &Path,
    runner: &dyn ToolRunner, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    if !runner.is_declared(&hint.value) {
        let finding = build_undeclared(rule, hint, *next_id);
        *next_id += 1;
        return Ok(vec![finding]);
    }
    let mut out: Vec<Diagnostic> = Vec::new();
    for candidate in candidates {
        let args = vec![candidate.to_string_lossy().into_owned()];
        let output = runner.run(&hint.value, &args, project_dir).map_err(|err| {
            HintError::ToolInvocation {
                rule_id: rule.rule_id.clone(),
                tool: hint.value.clone(),
                detail: err.to_string(),
            }
        })?;
        let parsed = parse_tool_findings(&output);
        if parsed.is_empty() && output.exit_code != 0 {
            let finding = build_invocation_failed(rule, hint, *next_id, candidate, &output.stderr);
            *next_id += 1;
            out.push(finding);
            continue;
        }
        for mut finding in parsed {
            restamp_finding(&mut finding, *next_id);
            *next_id += 1;
            out.push(finding);
        }
    }
    Ok(out)
}

fn parse_tool_findings(output: &ToolOutput) -> Vec<Diagnostic> {
    if output.stdout.is_empty() {
        return Vec::new();
    }
    if let Ok(envelope) = serde_json::from_slice::<DiagnosticReport>(&output.stdout) {
        return envelope.findings;
    }
    if let Ok(single) = serde_json::from_slice::<Diagnostic>(&output.stdout) {
        return vec![single];
    }
    Vec::new()
}

fn build_undeclared(rule: &ResolvedRule, hint: &DeterministicHint, id_num: u64) -> Diagnostic {
    let evidence = FindingEvidence::Snippet {
        value: format!("tool {tool} not declared by the project's tools.yaml", tool = hint.value),
    };
    make_synthetic_finding(SyntheticFinding {
        id_num,
        rule_id: "tool.undeclared",
        title: format!("Tool {} is not declared by the project", hint.value),
        severity: Severity::Important,
        location: None,
        evidence,
        impact: format!(
            "Rule {rule} cannot run; declared-tool gating refused the invocation.",
            rule = rule.rule_id
        ),
        remediation: format!("Declare {tool} in tools.yaml or remove the hint.", tool = hint.value),
        target_adapter: None,
    })
}

fn build_invocation_failed(
    rule: &ResolvedRule, hint: &DeterministicHint, id_num: u64, candidate: &Path, stderr: &[u8],
) -> Diagnostic {
    let snippet = clip_stderr(stderr);
    let evidence = FindingEvidence::Snippet { value: snippet };
    let location = FindingLocation {
        path: candidate.to_string_lossy().into_owned(),
        line: None,
        column: None,
        end_line: None,
        end_column: None,
    };
    make_synthetic_finding(SyntheticFinding {
        id_num,
        rule_id: "tool.invocation-failed",
        title: format!("Tool {} exited non-zero on {}", hint.value, candidate.display()),
        severity: rule.severity,
        location: Some(location),
        evidence,
        impact: format!(
            "Rule {rule} could not be evaluated; the declared tool exited non-zero.",
            rule = rule.rule_id
        ),
        remediation: format!("Inspect the tool stderr above and rerun {tool}.", tool = hint.value),
        target_adapter: None,
    })
}

fn clip_stderr(stderr: &[u8]) -> String {
    if stderr.len() <= STDERR_MAX_BYTES {
        return String::from_utf8_lossy(stderr).into_owned();
    }
    let mut out = String::from_utf8_lossy(&stderr[..STDERR_MAX_BYTES]).into_owned();
    out.push_str("…[truncated]");
    out
}
