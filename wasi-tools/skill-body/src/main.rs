//! `skill-body` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the body checks walk the whole `plugins/` tree) and reads
//! `PROJECT_DIR` from the environment. The positional args carry the
//! rule's own sentinel path (e.g. `…/CORE-040-…md`) and — when the rule
//! declares one — its `config:` serialised as JSON. The tool reads the
//! `CORE-NNN` out of the sentinel to scope its output to that one rule,
//! and reads its policy (line threshold, item bounds, built-in variable
//! allow-list) from the forwarded config, so no rule-specific literal is
//! baked into this binary.
//!
//! Findings are emitted on stdout as a `DiagnosticReport` envelope the
//! host folds into its scan output; each carries its own
//! `rule-id: CORE-NNN` and `severity: important`. The host restamps `id`
//! and `fingerprint`. Exit is always `0` on a successful run.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use serde_json::Value as JsonValue;
use specify_skill_body::{
    RULE_INVALID_CRITICAL_PATH, RULE_STEP_BODY_DUPLICATES, RULE_VARIABLE_COVERAGE, SkillBodyFinding,
    check_invalid_critical_path, check_step_body_duplicates, check_variable_coverage,
};

/// Placeholder fingerprint; the host recomputes it on fold.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

const RULES: &[&str] =
    &[RULE_INVALID_CRITICAL_PATH, RULE_STEP_BODY_DUPLICATES, RULE_VARIABLE_COVERAGE];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args);
    let config = parsed_config(&args);
    let findings = match scoped {
        Some(RULE_INVALID_CRITICAL_PATH) => check_invalid_critical_path(
            &project_dir,
            usize_field(config.as_ref(), "min-body-lines"),
            usize_field(config.as_ref(), "min-items"),
            usize_field(config.as_ref(), "max-items"),
        ),
        Some(RULE_STEP_BODY_DUPLICATES) => check_step_body_duplicates(&project_dir),
        Some(RULE_VARIABLE_COVERAGE) => {
            check_variable_coverage(&project_dir, &string_array_field(config.as_ref(), "builtin-vars"))
        }
        _ => Vec::new(),
    };
    print_report(&findings);
    ExitCode::SUCCESS
}

/// The single `CORE-NNN` named in the positional args (the rule's
/// sentinel file path), or `None` when no recognised rule is present.
fn requested_rule(args: &[String]) -> Option<&'static str> {
    args.iter().find_map(|arg| RULES.iter().copied().find(|rule| arg.contains(rule)))
}

/// The first positional arg that parses as a JSON object — the rule's
/// `config:` forwarded by the `kind: tool` evaluator.
fn parsed_config(args: &[String]) -> Option<JsonValue> {
    args.iter().find_map(|arg| match serde_json::from_str::<JsonValue>(arg) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    })
}

fn usize_field(config: Option<&JsonValue>, key: &str) -> usize {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

fn string_array_field(config: Option<&JsonValue>, key: &str) -> Vec<String> {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn print_report(findings: &[SkillBodyFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("skill-body: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[SkillBodyFinding]) -> Self {
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
    location: Location,
    evidence: Evidence,
    impact: String,
    remediation: String,
    fingerprint: String,
}

impl Finding {
    fn from_indexed((index, finding): (usize, &SkillBodyFinding)) -> Self {
        let (impact, remediation) = guidance(finding.rule_id);
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: finding.rule_id.to_string(),
            title: finding.message.clone(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location: Location { path: finding.path.clone() },
            evidence: Evidence::Snippet { value: finding.message.clone() },
            impact: impact.to_string(),
            remediation: remediation.to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
    }
}

fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_INVALID_CRITICAL_PATH => (
            "A long skill's `## Critical Path` section does not list the required number of steps, so the table of contents is not a faithful map of the skill body.",
            "Rewrite the `## Critical Path` section to list the configured number of bullets or numbered steps.",
        ),
        RULE_STEP_BODY_DUPLICATES => (
            "A step body repeats a `## Critical Path` entry verbatim, duplicating the table of contents instead of pointing to references.",
            "Keep step bodies as short pointers to references; do not restate the Critical Path entries.",
        ),
        _ => (
            "A skill defines a `$VAR` in its Arguments section that is never referenced (or references one that is never defined), so the variable contract is inconsistent.",
            "Reference every defined `$VAR` in the body and define every `$VAR` the body uses in the Arguments section.",
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
