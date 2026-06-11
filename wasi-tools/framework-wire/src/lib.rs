//! Shared stdout envelope for the framework-authoring WASI tools.
//!
//! Every framework checker (`scenarios`, `skill-body`, `agent-teams`,
//! `links-registry`, `marketplace`, `prose`, `rules`) emits the same
//! `DiagnosticReport` wire shape on stdout and parses the same
//! sentinel-path / forwarded-`config:` positional args. This crate owns
//! that envelope once: the serialize-only DTOs, the report printer, and
//! the arg/config helpers. Each tool keeps only its rule ids, its check
//! dispatch, and its per-rule guidance prose.
//!
//! The host folds the printed report into its own scan output and
//! restamps `id` and `fingerprint`; [`PLACEHOLDER_FINGERPRINT`] keeps the
//! envelope deserialisable until then. Deps stay `serde` / `serde_json`
//! only — this crate is part of the WASI carve-out and must never import
//! a host workspace crate.

use serde::Serialize;
use serde_json::Value as JsonValue;

/// Placeholder fingerprint; the host recomputes it on fold. Kept in the
/// `sha256:<64 hex>` wire shape so the envelope deserialises.
pub const PLACEHOLDER_FINGERPRINT: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// One tool finding plus its operator-facing guidance, ready to be
/// rendered into a wire [`Finding`]. Tools map their own finding type to
/// this row (typically pairing `rule_id` with a local `guidance()` fn).
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    /// Codex `CORE-NNN` id the finding belongs to.
    pub rule_id: &'a str,
    /// Operator-facing message describing the violation.
    pub message: &'a str,
    /// Project-relative, forward-slash path of the offending file, or
    /// `None` for whole-tree findings.
    pub path: Option<&'a str>,
    /// Operator-facing impact prose.
    pub impact: &'a str,
    /// Operator-facing remediation prose.
    pub remediation: &'a str,
}

/// Serialise the report built from `rows` and print it on stdout.
/// `tool` names the binary in the serialise-failure stderr line.
pub fn print_report<'a>(tool: &str, rows: impl IntoIterator<Item = Row<'a>>) {
    let report = Report::from_rows(rows);
    match serde_json::to_string(&report) {
        Ok(json) => println!("{json}"),
        Err(err) => eprintln!("{tool}: failed to serialise report: {err}"),
    }
}

/// The single rule id from `rules` named in the positional args (the
/// rule's sentinel file path), or `None` when no recognised rule is
/// present (direct local debugging emits the whole family).
#[must_use]
pub fn requested_rule(args: &[String], rules: &[&'static str]) -> Option<&'static str> {
    args.iter().find_map(|arg| rules.iter().copied().find(|rule| arg.contains(rule)))
}

/// The first positional arg that parses as a JSON object — the rule's
/// `config:` forwarded by the `kind: tool` evaluator.
#[must_use]
pub fn parsed_config(args: &[String]) -> Option<JsonValue> {
    args.iter().find_map(|arg| match serde_json::from_str::<JsonValue>(arg) {
        Ok(value) if value.is_object() => Some(value),
        _ => None,
    })
}

/// String field accessor over the forwarded config; empty when absent.
#[must_use]
pub fn string_field(config: Option<&JsonValue>, key: &str) -> String {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

/// Numeric field accessor over the forwarded config; `0` when absent.
#[must_use]
pub fn usize_field(config: Option<&JsonValue>, key: &str) -> usize {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

/// String-array field accessor over the forwarded config; empty when
/// absent.
#[must_use]
pub fn string_array_field(config: Option<&JsonValue>, key: &str) -> Vec<String> {
    config
        .and_then(|value| value.get(key))
        .and_then(JsonValue::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// `DiagnosticReport` stdout envelope.
#[derive(Serialize)]
pub struct Report {
    version: u8,
    summary: Summary,
    findings: Vec<Finding>,
}

impl Report {
    /// Build the wire report from finding rows. Every framework-tool
    /// finding is `severity: important`, so the summary counts rows
    /// there.
    pub fn from_rows<'a>(rows: impl IntoIterator<Item = Row<'a>>) -> Self {
        let wire: Vec<Finding> = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| Finding::from_indexed(index, row))
            .collect();
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
    fn from_indexed(index: usize, row: Row<'_>) -> Self {
        Self {
            id: format!("FIND-{:04}", index + 1),
            rule_id: row.rule_id.to_string(),
            title: row.message.to_string(),
            severity: "important".to_string(),
            source: "tool".to_string(),
            artifact: "unknown".to_string(),
            location: row.path.map(|path| Location { path: path.to_string() }),
            evidence: Evidence::Snippet {
                value: row.message.to_string(),
            },
            impact: row.impact.to_string(),
            remediation: row.remediation.to_string(),
            fingerprint: PLACEHOLDER_FINGERPRINT.to_string(),
        }
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

#[cfg(test)]
mod tests;
