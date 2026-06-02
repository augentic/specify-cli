//! Cross-repo schema-alias parity. The specify framework ships an
//! editor-facing mirror of the CLI's canonical `rule.schema.json` at
//! `.cursor/schemas/rule.schema.json`. The two are intentionally
//! *different shapes* — the canonical uses `oneOf` of `const` so a new
//! hint kind cannot land without an interpreter, the editor mirror uses
//! a flat `enum` so Cursor's JSON tooling completes the value — but the
//! closed hint-kind vocabulary they accept MUST stay identical. This
//! check is the only place that observes both at once: `specdev`
//! carries the canonical schema embedded in the binary, and lints the
//! framework root that owns the alias, so it enforces the cross-repo
//! seam at lint time. See REVIEW.md Part C.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;

const RULE_SCHEMA_ALIAS_HINT_KINDS: &str = "schema.alias-hint-kind-parity";

/// Framework-root-relative path to the editor-facing rule-schema mirror.
const ALIAS_REL: &str = ".cursor/schemas/rule.schema.json";

/// Assert the editor-facing `rule.schema.json` mirror accepts exactly
/// the canonical hint-kind vocabulary embedded in the CLI binary.
pub struct SchemaAliasCheck;

impl Check for SchemaAliasCheck {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        run_on_root(ctx.framework_root())
    }
}

/// Run the schema-alias parity predicate against a framework root (used
/// by integration tests). Roots without an alias mirror are skipped.
#[must_use]
pub fn run_on_root(root: &Path) -> Vec<Diagnostic> {
    let alias_path = root.join(ALIAS_REL);
    if !alias_path.is_file() {
        return Vec::new();
    }

    let canonical = match serde_json::from_str::<Value>(specify_schema::RULE_JSON_SCHEMA) {
        Ok(value) => hint_kinds(&value),
        // The embedded schema is validated elsewhere; a parse failure
        // here is not this check's concern.
        Err(_) => return Vec::new(),
    };

    let alias = match fs::read_to_string(&alias_path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => hint_kinds(&value),
            Err(source) => {
                return vec![framework_finding(
                    RULE_SCHEMA_ALIAS_HINT_KINDS,
                    format!("{ALIAS_REL} is not valid JSON: {source}"),
                    Some(loc(alias_path, 1, None)),
                )];
            }
        },
        Err(source) => {
            return vec![framework_finding(
                RULE_SCHEMA_ALIAS_HINT_KINDS,
                format!("{ALIAS_REL} could not be read: {source}"),
                Some(loc(alias_path, 1, None)),
            )];
        }
    };

    if alias == canonical {
        return Vec::new();
    }

    let missing: Vec<&str> = canonical.difference(&alias).map(String::as_str).collect();
    let extra: Vec<&str> = alias.difference(&canonical).map(String::as_str).collect();
    let mut detail = String::new();
    if !missing.is_empty() {
        detail.push_str(&format!(" missing from the alias: {}.", missing.join(", ")));
    }
    if !extra.is_empty() {
        detail.push_str(&format!(
            " present in the alias but unknown to the CLI: {}.",
            extra.join(", ")
        ));
    }

    vec![framework_finding(
        RULE_SCHEMA_ALIAS_HINT_KINDS,
        format!(
            "{ALIAS_REL} hint-kind vocabulary has drifted from the canonical CLI \
             rule.schema.json.{detail} Re-sync the editor mirror's `kind` enum with the \
             canonical `oneOf` of `const`.",
        ),
        Some(loc(alias_path, 1, None)),
    )]
}

/// Extract the closed `deterministic_hints[].kind` vocabulary from a
/// rule schema, accepting either the canonical `oneOf` of `const` shape
/// or the editor mirror's flat `enum` shape.
fn hint_kinds(schema: &Value) -> BTreeSet<String> {
    let kind = schema
        .pointer("/properties/deterministic_hints/items/properties/kind")
        .unwrap_or(&Value::Null);

    let mut out = BTreeSet::new();

    if let Some(values) = kind.get("enum").and_then(Value::as_array) {
        for value in values {
            if let Some(s) = value.as_str() {
                out.insert(s.to_owned());
            }
        }
    }

    if let Some(branches) = kind.get("oneOf").and_then(Value::as_array) {
        for branch in branches {
            if let Some(s) = branch.get("const").and_then(Value::as_str) {
                out.insert(s.to_owned());
            }
        }
    }

    out
}
