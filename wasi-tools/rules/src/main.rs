//! `rules` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the rule-tree checks are whole-tree) and reads `PROJECT_DIR`
//! from the environment. The positional args carry the rule's own
//! sentinel path (e.g. `…/CORE-009-…md`) and — for CORE-009 — its
//! `config:` serialised as JSON. The tool reads the `CORE-NNN` out of the
//! sentinel to scope its output to that one rule, and reads CORE-009's
//! owner→prefix policy (plus source-axis prefixes and reserved-namespace
//! owners) from the forwarded config, so no owner name, id-namespace
//! prefix, or reserved namespace is baked into this binary.
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-NNN` and `severity: important`. The host
//! restamps `id` and `fingerprint`. Exit is always `0` on a successful
//! run: the host treats a non-zero exit with no parsed findings as an
//! invocation failure, so a clean tree must exit `0`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::Value as JsonValue;
use specify_framework_wire::{Row, parsed_config, print_report, requested_rule};
use specify_rules::{
    OwnerPolicy, RULE_BODY_HEADING_MISSING, RULE_DUPLICATE_RULE_ID,
    RULE_NAMESPACE_OWNERSHIP_VIOLATION, RulesFinding, check_duplicate_rule_id,
    check_namespace_ownership, check_rule_body_heading,
};

/// Every codex id this tool can emit, scanned for in the positional args
/// to scope a single invocation to one rule.
const RULES: &[&str] =
    &[RULE_NAMESPACE_OWNERSHIP_VIOLATION, RULE_DUPLICATE_RULE_ID, RULE_BODY_HEADING_MISSING];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("rules", []);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args, RULES);
    let config = parsed_config(&args);

    let mut findings = Vec::new();
    if scoped.is_none() || scoped == Some(RULE_NAMESPACE_OWNERSHIP_VIOLATION) {
        // No owner policy supplied means nothing to compare against; emit
        // a clean report rather than treating every owner as unknown.
        if let Some(policy) = parse_policy(config.as_ref()) {
            findings.extend(check_namespace_ownership(&project_dir, &policy));
        }
    }
    if scoped.is_none() || scoped == Some(RULE_DUPLICATE_RULE_ID) {
        findings.extend(check_duplicate_rule_id(&project_dir));
    }
    if scoped.is_none() || scoped == Some(RULE_BODY_HEADING_MISSING) {
        findings.extend(check_rule_body_heading(&project_dir));
    }
    print_report("rules", findings.iter().map(row));
    ExitCode::SUCCESS
}

/// Build the CORE-009 namespace policy from the forwarded `config:`;
/// `None` when the required `owner-prefixes` map is absent. The engine
/// relays the value, the tool reads it.
fn parse_policy(config: Option<&JsonValue>) -> Option<OwnerPolicy> {
    let config = config?;
    let owner_prefixes = parse_prefix_map(config.get("owner-prefixes")?)?;
    let source_axis_prefixes =
        config.get("source-axis-prefixes").map(parse_string_set).unwrap_or_default();
    let reserved = config.get("reserved-namespaces").map(parse_string_map).unwrap_or_default();
    Some(OwnerPolicy {
        owner_prefixes,
        source_axis_prefixes,
        reserved,
    })
}

/// Parse an `{ owner: [prefix, …] }` object into the owner→prefix map.
fn parse_prefix_map(value: &JsonValue) -> Option<BTreeMap<String, BTreeSet<String>>> {
    let object = value.as_object()?;
    let mut map = BTreeMap::new();
    for (owner, prefixes) in object {
        map.insert(owner.clone(), parse_string_set(prefixes));
    }
    Some(map)
}

/// Parse a `{ key: value, … }` string object into a string→string map.
fn parse_string_map(value: &JsonValue) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(object) = value.as_object() {
        for (key, raw) in object {
            if let Some(text) = raw.as_str() {
                map.insert(key.clone(), text.to_string());
            }
        }
    }
    map
}

/// Parse a `[value, …]` string array into a set.
fn parse_string_set(value: &JsonValue) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Some(array) = value.as_array() {
        for raw in array {
            if let Some(text) = raw.as_str() {
                set.insert(text.to_string());
            }
        }
    }
    set
}

fn row(finding: &RulesFinding) -> Row<'_> {
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
        RULE_DUPLICATE_RULE_ID => (
            "The same rule id appears in more than one rules markdown file, so codex consumers cannot resolve a single rule.",
            "Rename the colliding rules so each frontmatter id is unique across the rules tree.",
        ),
        RULE_BODY_HEADING_MISSING => (
            "A rule markdown file's body is missing the `## Rule` heading, so reviewing agents cannot locate the policy text.",
            "Add a verbatim `## Rule` heading on its own line above the rule's policy statement.",
        ),
        _ => (
            "A rule's id-namespace prefix is not owned by the rules directory it lives under, so the codex namespace ownership invariant is broken.",
            "Move the rule into the directory that owns its namespace prefix, or renumber the id to the prefix its current directory owns.",
        ),
    }
}
