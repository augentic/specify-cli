//! Pure marketplace-manifest checks for the `marketplace`
//! framework-authoring tool, lifted from the host CLI's retiring
//! `framework::check::plugins` (`MarketplaceDriftCheck`) imperative
//! predicate (Road B framework tool).
//!
//! The tool covers CORE-022 (`plugins.marketplace-drift` — the
//! `.cursor-plugin/marketplace.json` manifest must satisfy its schema and
//! agree bidirectionally with the on-disk `plugins/` layout: every
//! declared plugin has a `skills/` directory and a `plugin.json`, and
//! every on-disk `plugin.json` is declared). This is a whole-tree,
//! bidirectional cross-fact check the indexer's per-file passes cannot
//! express.
//!
//! CORE-022 carries no policy — the manifest schema is embedded as a
//! byte-identical mechanism copy of `schemas/authoring/marketplace.schema.json`
//! and the plugin layout is structural. The only literals here are
//! mechanism (the manifest path, the `plugins/` and `.cursor-plugin/`
//! layout, the `skills/` directory name).
//!
//! Carve-out posture: this crate owns its logic and embeds its own copy
//! of `marketplace.schema.json`, depending only on `serde` / `serde_json`
//! / `jsonschema`, never the host diagnostics crate (`main.rs` renders
//! the wire envelope).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value as JsonValue;

/// Codex id every finding stamps (closed `CORE-NNN`).
pub const RULE_MARKETPLACE_DRIFT: &str = "CORE-022";

/// Tool-owned copy of the canonical marketplace manifest schema
/// (`schemas/authoring/marketplace.schema.json`). Embedded so the tool
/// never reaches back into the host engine for policy (Road B B-2).
const MARKETPLACE_SCHEMA_SOURCE: &str = include_str!("../embedded/marketplace.schema.json");

/// One marketplace-drift violation: its codex `rule_id`, an optional
/// project-relative path, and a human-readable message. The caller
/// stamps the wire severity (always `important`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file.
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// CORE-022: validate the marketplace manifest shape and its consistency
/// with the on-disk plugin layout rooted at `project_dir`.
#[must_use]
pub fn check_marketplace_drift(project_dir: &Path) -> Vec<MarketplaceFinding> {
    let manifest_path = project_dir.join(".cursor-plugin").join("marketplace.json");
    let manifest_rel = relative_display(project_dir, &manifest_path);

    let Ok(contents) = std::fs::read_to_string(&manifest_path) else {
        return vec![drift(&manifest_rel, "Cannot read .cursor-plugin/marketplace.json")];
    };

    let value: JsonValue = match serde_json::from_str(&contents) {
        Ok(value) => value,
        Err(error) => {
            return vec![drift(
                &manifest_rel,
                &format!("Cannot parse .cursor-plugin/marketplace.json: {error}"),
            )];
        }
    };

    let schema_findings = schema_findings(&value, &manifest_rel);
    if !schema_findings.is_empty() {
        return schema_findings;
    }

    let Some(plugins) = value.get("plugins").and_then(JsonValue::as_array) else {
        return vec![drift(&manifest_rel, "marketplace.json is missing a plugins array")];
    };

    let declared: BTreeSet<String> = plugins
        .iter()
        .filter_map(|entry| entry.get("source").and_then(JsonValue::as_str))
        .map(str::to_string)
        .collect();

    let plugins_dir = project_dir.join("plugins");
    let mut findings = undeclared_findings(project_dir, &plugins_dir, &declared);

    for plugin in plugins {
        let Some(name) = plugin.get("name").and_then(JsonValue::as_str) else {
            continue;
        };
        let Some(source) = plugin.get("source").and_then(JsonValue::as_str) else {
            continue;
        };

        let plugin_root = plugins_dir.join(source);
        let skills_dir = plugin_root.join("skills");
        match std::fs::metadata(&skills_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                findings
                    .push(drift(&manifest_rel, &format!("Plugin '{name}' has no skills/ directory")));
                continue;
            }
            Err(_) => {
                findings.push(drift(
                    &manifest_rel,
                    &format!("Plugin '{name}' declared in marketplace.json but skills/ not found"),
                ));
                continue;
            }
        }

        let plugin_manifest = plugin_root.join(".cursor-plugin").join("plugin.json");
        let plugin_manifest_rel = relative_display(project_dir, &plugin_manifest);
        match std::fs::metadata(&plugin_manifest) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => findings.push(drift(
                &plugin_manifest_rel,
                &format!("Plugin '{name}' has skills/ but .cursor-plugin/plugin.json is not a file"),
            )),
            Err(_) => findings.push(drift(
                &plugin_manifest_rel,
                &format!("Plugin '{name}' has skills/ but .cursor-plugin/plugin.json not found"),
            )),
        }
    }

    findings
}

/// Validate the parsed manifest against the embedded marketplace schema,
/// returning one finding per constraint violation (or an infrastructure
/// finding when the schema itself cannot compile).
fn schema_findings(value: &JsonValue, manifest_rel: &str) -> Vec<MarketplaceFinding> {
    let validator = match marketplace_validator() {
        Ok(validator) => validator,
        Err(error) => {
            return vec![drift(
                manifest_rel,
                &format!("Cannot validate .cursor-plugin/marketplace.json: {error}"),
            )];
        }
    };
    if validator.is_valid(value) {
        return Vec::new();
    }
    validator
        .iter_errors(value)
        .map(|error| {
            drift(
                manifest_rel,
                &format!(
                    "marketplace.json schema violation at {}: {}",
                    error.instance_path(),
                    error
                ),
            )
        })
        .collect()
}

fn marketplace_validator() -> Result<&'static jsonschema::Validator, String> {
    static VALIDATOR: OnceLock<Result<jsonschema::Validator, String>> = OnceLock::new();
    VALIDATOR
        .get_or_init(|| {
            let schema: JsonValue = serde_json::from_str(MARKETPLACE_SCHEMA_SOURCE)
                .map_err(|err| format!("embedded marketplace.schema.json is not JSON: {err}"))?;
            jsonschema::validator_for(&schema)
                .map_err(|err| format!("embedded marketplace.schema.json failed to compile: {err}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Flag every on-disk `plugins/<plugin>/.cursor-plugin/plugin.json` whose
/// `<plugin>` directory name is not a declared `source`.
fn undeclared_findings(
    project_dir: &Path, plugins_dir: &Path, declared: &BTreeSet<String>,
) -> Vec<MarketplaceFinding> {
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return Vec::new();
    };
    let mut findings = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let manifest = entry.path().join(".cursor-plugin").join("plugin.json");
        if !manifest.is_file() {
            continue;
        }
        if !declared.contains(&name) {
            let rel = relative_display(project_dir, &manifest);
            findings.push(drift(
                &rel,
                &format!(
                    "Plugin '{name}' has .cursor-plugin/plugin.json but is not in marketplace.json"
                ),
            ));
        }
    }
    findings.sort_by(|a, b| a.message.cmp(&b.message));
    findings
}

fn drift(rel: &str, message: &str) -> MarketplaceFinding {
    MarketplaceFinding {
        rule_id: RULE_MARKETPLACE_DRIFT,
        path: Some(rel.to_string()),
        message: message.to_string(),
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MANIFEST: &str = r#"{
  "name": "test",
  "owner": { "name": "Test Owner", "email": "test@example.com" },
  "metadata": { "description": "Synthetic.", "version": "0.0.0", "pluginRoot": "plugins" },
  "plugins": [
    { "name": "spec", "source": "spec", "description": "Spec plugin." }
  ]
}
"#;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    fn write_plugin(root: &Path, source: &str) {
        write(root, &format!("plugins/{source}/skills/.keep"), "");
        write(root, &format!("plugins/{source}/.cursor-plugin/plugin.json"), "{\"name\":\"x\"}");
    }

    #[test]
    fn clean_tree_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), ".cursor-plugin/marketplace.json", VALID_MANIFEST);
        write_plugin(dir.path(), "spec");
        assert!(check_marketplace_drift(dir.path()).is_empty());
    }

    #[test]
    fn flags_undeclared_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), ".cursor-plugin/marketplace.json", VALID_MANIFEST);
        write_plugin(dir.path(), "spec");
        write_plugin(dir.path(), "orphan");
        let findings = check_marketplace_drift(dir.path());
        assert!(
            findings.iter().any(|f| f.message.contains("orphan")
                && f.message.contains("not in marketplace.json")),
            "expected undeclared plugin finding, got {findings:?}"
        );
    }

    #[test]
    fn flags_schema_violation() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            ".cursor-plugin/marketplace.json",
            "{\n  \"name\": \"bad\",\n  \"plugins\": []\n}\n",
        );
        let findings = check_marketplace_drift(dir.path());
        assert!(
            findings.iter().all(|f| f.rule_id == RULE_MARKETPLACE_DRIFT),
            "all findings carry CORE-022"
        );
        assert!(
            findings.iter().any(|f| f.message.contains("schema violation")),
            "expected schema violation finding, got {findings:?}"
        );
    }

    #[test]
    fn flags_declared_plugin_missing_skills() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), ".cursor-plugin/marketplace.json", VALID_MANIFEST);
        write(dir.path(), "plugins/spec/.cursor-plugin/plugin.json", "{\"name\":\"x\"}");
        let findings = check_marketplace_drift(dir.path());
        assert!(
            findings.iter().any(|f| f.message.contains("skills/ not found")),
            "expected missing-skills finding, got {findings:?}"
        );
    }
}
