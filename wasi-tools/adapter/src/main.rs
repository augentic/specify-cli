//! `adapter` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the adapter-structure checks are whole-tree) and reads
//! `PROJECT_DIR` from the environment. The positional args carry the
//! rule's own sentinel path (e.g. `…/CORE-049-…md`) and — when the rule
//! declares one — its `config:` serialised as JSON. The tool reads the
//! `CORE-NNN` out of the sentinel to scope its output to that one rule
//! and reads CORE-049's `{adapter, tool, package}` policy table from the
//! forwarded config, so no rule-specific literal is baked into this
//! binary.
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
use specify_adapter::{
    AdapterFinding, ExpectedTool, RULE_INVALID_DECLARATION, RULE_MISSING_MANIFEST,
    check_invalid_declaration, check_missing_manifest,
};

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Every codex id this tool can emit, scanned for in the positional args
/// to scope a single invocation to one rule.
const RULES: &[&str] = &[RULE_MISSING_MANIFEST, RULE_INVALID_DECLARATION];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report(&[]);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args);
    let config = parsed_config(&args);

    let mut findings = Vec::new();
    if scoped.is_none() || scoped == Some(RULE_MISSING_MANIFEST) {
        findings.extend(check_missing_manifest(&project_dir));
    }
    if scoped.is_none() || scoped == Some(RULE_INVALID_DECLARATION) {
        let expected = expected_tools(config.as_ref());
        findings.extend(check_invalid_declaration(&project_dir, &expected));
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

/// Parse CORE-049's `{adapter, tool, package}` policy rows out of the
/// forwarded `config.expected-tools` array. Rows missing any field are
/// dropped; the engine relays the value, the tool reads it.
fn expected_tools(config: Option<&JsonValue>) -> Vec<ExpectedTool> {
    config
        .and_then(|value| value.get("expected-tools"))
        .and_then(JsonValue::as_array)
        .map(|rows| rows.iter().filter_map(expected_row).collect())
        .unwrap_or_default()
}

fn expected_row(row: &JsonValue) -> Option<ExpectedTool> {
    let adapter = row.get("adapter").and_then(JsonValue::as_str)?;
    let tool = row.get("tool").and_then(JsonValue::as_str)?;
    let package = row.get("package").and_then(JsonValue::as_str)?;
    Some(ExpectedTool {
        adapter: adapter.to_string(),
        name: tool.to_string(),
        package: package.to_string(),
    })
}

fn print_report(findings: &[AdapterFinding]) {
    let report = Report::from_findings(findings);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("adapter: failed to serialise report: {err}"),
    }
}

#[derive(Serialize)]
struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    fn from_findings(findings: &[AdapterFinding]) -> Self {
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
    fn from_indexed((index, finding): (usize, &AdapterFinding)) -> Self {
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
        RULE_MISSING_MANIFEST => (
            "An adapter directory has no adapter.yaml manifest, so the loader cannot resolve it.",
            "Add an adapter.yaml manifest to the adapter directory, or remove the stray directory.",
        ),
        _ => (
            "A target adapter's first-party tool declaration does not match the pinned policy table, so the wrong tool artifact could be resolved.",
            "Declare each first-party tool under `tools[]` with the exact pinned name and version from the rule's policy table.",
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
