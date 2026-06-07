//! `links-registry` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the link-registry checks are whole-tree) and reads `PROJECT_DIR`
//! from the environment. The positional args carry the rule's own
//! sentinel path (e.g. `…/CORE-018-…md`) and — when the rule declares one
//! — its `config:` serialised as JSON. The tool reads the `CORE-NNN` out
//! of the sentinel to scope its output to that one rule and reads
//! CORE-018's tool→schema registry from the forwarded config, so no
//! rule-specific literal is baked into this binary.
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
use specify_links_registry::{
    KnownSchema, LinkFinding, RULE_BRIEF_SCHEMA_LINK_RESOLVE, RULE_UNRESOLVED_DIRECTIVE,
    check_directives, check_schema_links,
};

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Every codex id this tool can emit, scanned for in the positional args
/// to scope a single invocation to one rule.
const RULES: &[&str] = &[RULE_BRIEF_SCHEMA_LINK_RESOLVE, RULE_UNRESOLVED_DIRECTIVE];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args);
    let config = parsed_config(&args);

    let mut findings = Vec::new();
    if scoped.is_none() || scoped == Some(RULE_BRIEF_SCHEMA_LINK_RESOLVE) {
        let registry = known_schemas(config.as_ref());
        findings.extend(check_schema_links(&project_dir, &registry));
    }
    if scoped.is_none() || scoped == Some(RULE_UNRESOLVED_DIRECTIVE) {
        findings.extend(check_directives(&project_dir));
    }
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

/// Parse CORE-018's tool→schema registry out of the forwarded
/// `config.known-schemas` array. Rows missing fields are dropped; the
/// engine relays the value, the tool reads it.
fn known_schemas(config: Option<&JsonValue>) -> Vec<KnownSchema> {
    config
        .and_then(|value| value.get("known-schemas"))
        .and_then(JsonValue::as_array)
        .map(|rows| rows.iter().filter_map(known_schema_row).collect())
        .unwrap_or_default()
}

fn known_schema_row(row: &JsonValue) -> Option<KnownSchema> {
    let tool = row.get("tool").and_then(JsonValue::as_str)?;
    let schemas = row
        .get("schemas")
        .and_then(JsonValue::as_array)?
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect();
    Some(KnownSchema { tool: tool.to_string(), schemas })
}

fn print_report(findings: &[LinkFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("links-registry: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[LinkFinding]) -> Self {
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
    fn from_indexed((index, finding): (usize, &LinkFinding)) -> Self {
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

/// Per-rule operator-facing impact / remediation prose.
fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_BRIEF_SCHEMA_LINK_RESOLVE => (
            "An adapter brief references a schemas.specify.dev URL that does not resolve to a known tool-owned schema, so readers follow a dead link.",
            "Point the URL at a schema named in the rule's known-schemas registry, or register the schema with its owning tool first.",
        ),
        _ => (
            "A skill directive references a plugin or skill that does not exist on disk, so the directive cannot resolve at runtime.",
            "Fix the `<!-- skill: plugin:skill -->` directive to name an existing plugin and skill under plugins/.",
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
