//! `scenarios` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since scenario checks are whole-tree) and reads `PROJECT_DIR` from the
//! environment. The positional args carry the rule's own sentinel file
//! (e.g. `…/CORE-028-…md`) and — when the rule declares one — its
//! `config:` serialised as JSON. The tool reads the `CORE-NNN` out of
//! the sentinel and scopes its output to that rule, so the family tool
//! can back CORE-028..033 and CORE-056 without each rule
//! double-counting the others' findings; CORE-056's catalog↔runs policy
//! (paths, value sets, status↔result map) rides in the forwarded
//! config, never this binary. With no recognisable rule in the args the
//! tool emits the whole family (direct local debugging).
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-NNN` and `severity: important`. The host
//! restamps `id` and `fingerprint`.
//!
//! Exit is always `0` on a successful run: the host treats a non-zero
//! exit with no parsed findings as an invocation failure, so a clean tree
//! must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use specify_framework_wire::{Row, parsed_config, print_report, requested_rule};
use specify_scenarios::{
    RULE_ARTIFACT_PATH_UNSAFE, RULE_BODY_ID_MISMATCH, RULE_CATALOG_RUNS_DRIFT,
    RULE_RECORDED_TRACE_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS, ScenarioFinding, run_with_config,
};

/// Every codex id this tool can emit, scanned for in the positional
/// args to scope a single invocation to one rule.
const RULES: &[&str] = &[
    RULE_ARTIFACT_PATH_UNSAFE,
    RULE_BODY_ID_MISMATCH,
    RULE_CATALOG_RUNS_DRIFT,
    RULE_RECORDED_TRACE_VIOLATION,
    RULE_STAGES_NOT_CONTIGUOUS,
];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("scenarios", []);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args, RULES);
    let config = parsed_config(&args);
    let findings: Vec<ScenarioFinding> = run_with_config(&project_dir, config.as_ref())
        .into_iter()
        .filter(|finding| scoped.is_none_or(|rule| finding.rule_id == rule))
        .collect();
    print_report("scenarios", findings.iter().map(row));
    ExitCode::SUCCESS
}

fn row(finding: &ScenarioFinding) -> Row<'_> {
    let (impact, remediation) = guidance(finding.rule_id);
    Row {
        rule_id: finding.rule_id,
        message: &finding.message,
        path: finding.path.as_deref(),
        impact,
        remediation,
    }
}

/// Per-rule operator-facing impact / remediation prose.
fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_ARTIFACT_PATH_UNSAFE => (
            "A scenario declares an expected artifact path that is empty, absolute, or escapes the scenario workspace.",
            "Rewrite each `expected-artifacts` entry as a non-empty path relative to the scenario workspace, with no leading '/' or '..' segments.",
        ),
        RULE_BODY_ID_MISMATCH => (
            "A scenario's visible 'Scenario ID' body line disagrees with its frontmatter id, so readers cannot trust the citation.",
            "Align the body 'Scenario ID: `…`' line with the frontmatter `id`.",
        ),
        RULE_RECORDED_TRACE_VIOLATION => (
            "A recorded-trace file's first line is not a well-formed `recorded-trace-header`, so replay cannot trust its provenance.",
            "Make the first line a JSON `recorded-trace-header` object with schemaVersion 1 and every required field populated.",
        ),
        RULE_CATALOG_RUNS_DRIFT => (
            "The scenario catalog, the scenario files, and the committed run records disagree, so the catalog's gate status cannot be trusted.",
            "Reconcile the catalog row with the scenario tree and evals/runs/: status-bearing rows need exactly one committed record whose <result> agrees.",
        ),
        _ => (
            "Scenario stages are not a contiguous slice of the slice loop; the pack does not describe a runnable lifecycle window.",
            "Reorder the scenario's `stages` list to a contiguous run of [plan, refine, build, merge, drop] anchored at any element.",
        ),
    }
}
