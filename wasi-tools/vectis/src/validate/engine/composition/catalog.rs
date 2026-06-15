//! Component-catalog cross-reference (component catalog contract):
//! catalog parsing plus the slug↔entry agreement checks.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use super::finding::Finding;
use crate::validate::engine::shared::escape_pointer_token;

/// Read and parse the component catalog with explicit error reporting.
/// Unlike `parse_yaml_file` (which returns `None` silently because its
/// callers have an auto-invoked sibling validator), the catalog has no
/// prior validation step — a present-but-invalid file must surface as
/// a composition-mode error rather than being silently skipped.
pub(super) fn parse_catalog_file(path: &Path) -> Result<Value, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|err| format!("component catalog at {} is not readable: {err}", path.display()))?;
    serde_saphyr::from_str::<Value>(&source).map_err(|err| {
        format!("component catalog at {} contains invalid YAML: {err}", path.display())
    })
}

/// Cross-reference every `component: <slug>` in the composition
/// against the agent-inferred, operator-reviewable component catalog.
///
/// - A slug absent from the catalog → error.
/// - A slug with `status: rejected` → error.
/// - A slug with `status: confirmed` → OK.
/// - A confirmed catalog entry with zero `component:` references in
///   the composition → warning (the entry exists but is unused in
///   this artifact).
pub(super) fn check_catalog_cross_references(
    instance: &Value, catalog: &Value, errors: &mut Vec<Finding>, warnings: &mut Vec<Finding>,
) {
    let Some(components) = catalog.get("components").and_then(Value::as_object) else {
        return;
    };

    let mut slug_refs: Vec<(String, String)> = Vec::new();
    collect_component_slugs(instance, "", &mut slug_refs);

    for (slug, path) in &slug_refs {
        match components.get(slug.as_str()) {
            None => {
                errors.push(Finding::new(
                    path.clone(),
                    format!("component slug `{slug}` is not present in the component catalog"),
                ));
            }
            Some(entry) => {
                if entry.get("status").and_then(Value::as_str) == Some("rejected") {
                    errors.push(Finding::new(
                        path.clone(),
                        format!(
                            "component slug `{slug}` has `status: rejected` in the component catalog",
                        ),
                    ));
                }
            }
        }
    }

    let used: BTreeSet<&str> = slug_refs.iter().map(|(s, _)| s.as_str()).collect();
    for (slug, entry) in components {
        if entry.get("status").and_then(Value::as_str) == Some("confirmed")
            && !used.contains(slug.as_str())
        {
            warnings.push(Finding::new(
                "",
                format!(
                    "confirmed catalog entry `{slug}` has no `component: {slug}` reference in composition.yaml",
                ),
            ));
        }
    }
}

/// Walk a composition document and collect every `component: <slug>`
/// directive as a `(slug, json_pointer_path)` pair.
fn collect_component_slugs(node: &Value, json_path: &str, out: &mut Vec<(String, String)>) {
    match node {
        Value::Object(map) => {
            for (key, val) in map {
                let child_path = format!("{json_path}/{}", escape_pointer_token(key));
                if key == "group"
                    && let Some(slug) = val.get("component").and_then(Value::as_str)
                {
                    out.push((slug.to_string(), child_path.clone()));
                }
                collect_component_slugs(val, &child_path, out);
            }
        }
        Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                collect_component_slugs(v, &format!("{json_path}/{i}"), out);
            }
        }
        _ => {}
    }
}
