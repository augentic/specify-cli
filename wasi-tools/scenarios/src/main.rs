//! `scenarios` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since scenario checks are whole-tree) and reads `PROJECT_DIR` from the
//! environment. The positional path argument names the rule's own
//! sentinel file (e.g. `…/CORE-028-…md`); the tool reads the `CORE-NNN`
//! out of it and scopes its output to that rule, so the family tool can
//! back CORE-028..033 without each rule double-counting the others'
//! findings. With no recognisable rule in the args the tool emits the
//! whole family (direct local debugging).
//!
//! Findings are emitted on stdout as a `DiagnosticReport` envelope the
//! host folds into its scan output; each carries its own
//! `rule-id: CORE-NNN` and `severity: important`. The host restamps `id`
//! and `fingerprint`.
//!
//! Exit is always `0` on a successful run: the host treats a non-zero
//! exit with no parsed findings as an invocation failure, so a clean tree
//! must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use specify_scenarios::{
    RULE_ARTIFACT_PATH_UNSAFE, RULE_BODY_ID_MISMATCH, RULE_DUPLICATE_ID,
    RULE_RECORDED_TRACE_VIOLATION, RULE_SCHEMA_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS,
    ScenarioFinding, run,
};

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
const PLACEHOLDER_FINGERPRINT: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Every codex id this tool can emit, scanned for in the positional
/// args to scope a single invocation to one rule.
const RULES: &[&str] = &[
    RULE_ARTIFACT_PATH_UNSAFE,
    RULE_BODY_ID_MISMATCH,
    RULE_DUPLICATE_ID,
    RULE_RECORDED_TRACE_VIOLATION,
    RULE_SCHEMA_VIOLATION,
    RULE_STAGES_NOT_CONTIGUOUS,
];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let scoped = requested_rule();
    let findings: Vec<ScenarioFinding> = run(&project_dir)
        .into_iter()
        .filter(|finding| scoped.is_none_or(|rule| finding.rule_id == rule))
        .collect();
    print_report(&findings);
    ExitCode::SUCCESS
}

/// The single `CORE-NNN` named in the positional args (the rule's
/// sentinel file path), or `None` when no recognised rule is present.
fn requested_rule() -> Option<&'static str> {
    std::env::args().find_map(|arg| RULES.iter().copied().find(|rule| arg.contains(rule)))
}

fn print_report(findings: &[ScenarioFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("scenarios: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[ScenarioFinding]) -> Self {
        let wire: Vec<Finding> = findings.iter().enumerate().map(Finding::from_indexed).collect();
        Self {
            version: 1,
            summary: Summary {
                critical: 0,
                important: u32::try_from(wire.len()).unwrap_or(u32::MAX),
                suggestion: 0,
                optional: 0,
            },
            findings: wire,
        }
    }
}

#[derive(Serialize)]
struct Summary {
    critical: u32,
    important: u32,
    suggestion: u32,
    optional: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Finding {
    id: String,
    rule_id: String,
    title: String,
    severity: String,
    source: String,
    artifact: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
    evidence: Evidence,
    impact: String,
    remediation: String,
    fingerprint: String,
}

impl Finding {
    fn from_indexed((index, finding): (usize, &ScenarioFinding)) -> Self {
        let (impact, remediation) = guidance(finding.rule_id);
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: finding.rule_id.to_string(),
            title: finding.message.clone(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location: finding.path.clone().map(|path| Location { path }),
            evidence: Evidence::Snippet {
                value: finding.message.clone(),
            },
            impact: impact.to_string(),
            remediation: remediation.to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
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
        RULE_DUPLICATE_ID => (
            "Two or more scenarios share an id; scenario ids must be unique across the whole tree.",
            "Rename the colliding scenarios so each frontmatter `id` is unique.",
        ),
        RULE_RECORDED_TRACE_VIOLATION => (
            "A recorded-trace file's first line is not a well-formed `recorded-trace-header`, so replay cannot trust its provenance.",
            "Make the first line a JSON `recorded-trace-header` object with schemaVersion 1 and every required field populated.",
        ),
        RULE_SCHEMA_VIOLATION => (
            "A scenario's frontmatter does not satisfy scenario.schema.json, so it cannot be consumed by the acceptance tooling.",
            "Fix the scenario frontmatter to satisfy scenario.schema.json (see the finding message for the failing field).",
        ),
        _ => (
            "Scenario stages are not a contiguous slice of the slice loop; the pack does not describe a runnable lifecycle window.",
            "Reorder the scenario's `stages` list to a contiguous run of [plan, refine, build, merge, drop] anchored at any element.",
        ),
    }
}

#[derive(Serialize)]
struct Location {
    path: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Evidence {
    Snippet { value: String },
}
