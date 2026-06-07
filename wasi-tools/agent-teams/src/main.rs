//! `agent-teams` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the overlay checks are whole-tree) and reads `PROJECT_DIR` from
//! the environment. The positional args carry the rule's own sentinel
//! path (e.g. `…/CORE-012-…md`) and its `config:` serialised as JSON. The
//! tool reads the `CORE-NNN` out of the sentinel to scope its output to
//! that one rule, and reads the canonical-document path (its only policy)
//! from the forwarded config, so no rule-specific literal is baked into
//! this binary.
//!
//! Findings are emitted on stdout as a `DiagnosticReport` envelope the
//! host folds into its scan output; each carries its own
//! `rule-id: CORE-NNN` and `severity: important`. The host restamps `id`
//! and `fingerprint`. Exit is always `0` on a successful run: the host
//! treats a non-zero exit with no parsed findings as an invocation
//! failure, so a clean tree must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use serde_json::Value as JsonValue;
use specify_agent_teams::{AgentTeamsFinding, RULE_NON_CANONICAL, run};

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Every codex id this tool can emit, scanned for in the positional args
/// to scope a single invocation to one rule.
const RULES: &[&str] = &[RULE_NON_CANONICAL];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args);
    let config = parsed_config(&args);
    let canonical_rel = string_field(config.as_ref(), "canonical-path");
    let findings: Vec<AgentTeamsFinding> = if canonical_rel.is_empty() {
        Vec::new()
    } else {
        run(&project_dir, &canonical_rel)
            .into_iter()
            .filter(|finding| scoped.is_none_or(|rule| finding.rule_id == rule))
            .collect()
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

fn string_field(config: Option<&JsonValue>, key: &str) -> String {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

fn print_report(findings: &[AgentTeamsFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("agent-teams: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[AgentTeamsFinding]) -> Self {
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
    fn from_indexed((index, finding): (usize, &AgentTeamsFinding)) -> Self {
        let (impact, remediation) = guidance(finding.rule_id);
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: finding.rule_id.to_string(),
            title: finding.message.clone(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location: finding.path.clone().map(|path| Location { path }),
            evidence: Evidence::Snippet { value: finding.message.clone() },
            impact: impact.to_string(),
            remediation: remediation.to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
    }
}

/// Operator-facing impact / remediation prose for CORE-012 overlay
/// drift (the only rule this tool now emits).
fn guidance(_rule_id: &str) -> (&'static str, &'static str) {
    (
        "A target adapter's agent-teams.md overlay does not match the canonical review-team-protocol document, so the overlay has drifted.",
        "Replace the overlay with a symlink to the canonical document, or re-sync its contents so the digests match.",
    )
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
