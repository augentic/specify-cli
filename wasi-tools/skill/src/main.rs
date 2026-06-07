//! `skill` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the frontmatter checks walk the whole `plugins/` tree) and reads
//! `PROJECT_DIR` from the environment. The positional args carry the
//! rule's own sentinel path (e.g. `…/CORE-036-…md`) and — when the rule
//! declares one — its `config:` serialised as JSON. The tool reads the
//! `CORE-NNN` out of the sentinel to scope its output to that one rule
//! and reads its policy (argument-hint grammar, description-verb
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
use specify_skill::{
    RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_MISSING_FRONTMATTER, SkillFinding,
    check_argument_hint_grammar, check_description_grammar, check_missing_frontmatter,
};

/// Placeholder fingerprint; the host recomputes it on fold.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

const RULES: &[&str] =
    &[RULE_MISSING_FRONTMATTER, RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args);
    let config = parsed_config(&args);
    let findings = match scoped {
        Some(RULE_MISSING_FRONTMATTER) => check_missing_frontmatter(&project_dir),
        Some(RULE_ARGUMENT_HINT_GRAMMAR) => {
            check_argument_hint_grammar(&project_dir, &string_field(config.as_ref(), "token-pattern"))
        }
        Some(RULE_DESCRIPTION_GRAMMAR) => {
            check_description_grammar(&project_dir, &string_array_field(config.as_ref(), "allowed-verbs"))
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

fn string_field(config: Option<&JsonValue>, key: &str) -> String {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

fn string_array_field(config: Option<&JsonValue>, key: &str) -> Vec<String> {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn print_report(findings: &[SkillFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("skill: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[SkillFinding]) -> Self {
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
    fn from_indexed((index, finding): (usize, &SkillFinding)) -> Self {
        let (impact, remediation) = guidance(finding.rule_id);
        let location = if finding.path.is_empty() {
            None
        } else {
            Some(Location { path: finding.path.clone() })
        };
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: finding.rule_id.to_string(),
            title: finding.message.clone(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location,
            evidence: Evidence::Snippet { value: finding.message.clone() },
            impact: impact.to_string(),
            remediation: remediation.to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
    }
}

fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_MISSING_FRONTMATTER => (
            "A SKILL.md has no parseable leading YAML frontmatter, so the runtime cannot register the skill.",
            "Add a leading `---` … `---` YAML frontmatter block with the required `name` and `description` keys.",
        ),
        RULE_ARGUMENT_HINT_GRAMMAR => (
            "A skill's `argument-hint` contains a token that does not match the slash-command argument grammar, so the hint cannot be rendered.",
            "Rewrite each `argument-hint` token using the closed grammar (`<name>`, `[name]`, `<a|b>`, `--flag`, with optional `...`).",
        ),
        _ => (
            "A skill's `description` does not start with an approved imperative verb, so it reads inconsistently with the rest of the skill catalog.",
            "Begin the `description` with an imperative verb from the approved allow-list.",
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
