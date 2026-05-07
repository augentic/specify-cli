//! Structural validation for declared Specify WASI tools.

use std::collections::HashSet;
use std::path::Path;

use crate::manifest::{Tool, ToolManifest, ToolScope, ToolSource};

/// Canonical JSON Schema for the two `tools:` declaration sites.
pub const TOOL_JSON_SCHEMA: &str = include_str!("../../../schemas/tool.schema.json");

const RULE_NAME_FORMAT: &str = "tool.name-format";
const RULE_VERSION_SEMVER: &str = "tool.version-is-semver";
const RULE_SOURCE_SUPPORTED: &str = "tool.source-is-supported-uri";
const RULE_SHA256_FORMAT: &str = "tool.sha256-format";
const RULE_PERMISSION_PATH_FORM: &str = "tool.permission-path-form";
const RULE_LIFECYCLE_WRITE_DENIED: &str = "tool.lifecycle-state-write-denied";
const RULE_CAPABILITY_DIR_SCOPE: &str = "tool.capability-dir-out-of-scope";
const RULE_NAME_UNIQUE: &str = "tool.name-unique";

/// Outcome of a structural validation rule.
///
/// This mirrors `specify_capability::ValidationResult` locally so
/// `specify-tool` does not depend on `specify-capability`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationResult {
    /// Rule passed.
    Pass {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
    },
    /// Rule failed.
    Fail {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
        /// Detail message explaining the failure.
        detail: String,
    },
    /// Rule evaluation was deferred.
    Deferred {
        /// Machine-readable rule identifier.
        rule_id: &'static str,
        /// Human-readable rule description.
        rule: &'static str,
        /// Why the rule was deferred.
        reason: &'static str,
    },
}

impl ValidationResult {
    /// Stable rule identifier for this result.
    #[must_use]
    pub const fn rule_id(&self) -> &'static str {
        match self {
            Self::Pass { rule_id, .. }
            | Self::Fail { rule_id, .. }
            | Self::Deferred { rule_id, .. } => rule_id,
        }
    }
}

impl Tool {
    /// Validate the tool declaration against RFC-15's chunk-1 structural rules.
    #[must_use]
    pub fn validate_structure(&self, scope: &ToolScope) -> Vec<ValidationResult> {
        vec![
            validate_name(&self.name),
            validate_version(&self.version),
            validate_source(&self.source),
            validate_sha256(self.sha256.as_deref()),
            validate_permission_paths(&self.permissions.read, &self.permissions.write),
            validate_lifecycle_writes(&self.permissions.write),
            validate_capability_dir_scope(scope, &self.permissions.read, &self.permissions.write),
        ]
    }
}

impl ToolManifest {
    /// Validate a manifest and all of its contained tools.
    #[must_use]
    pub fn validate_structure(&self, scope: &ToolScope) -> Vec<ValidationResult> {
        let mut results = Vec::with_capacity(1 + self.tools.len() * 7);
        results.push(validate_unique_names(&self.tools));
        for tool in &self.tools {
            results.extend(tool.validate_structure(scope));
        }
        results
    }
}

const fn pass(rule_id: &'static str, rule: &'static str) -> ValidationResult {
    ValidationResult::Pass { rule_id, rule }
}

fn fail(rule_id: &'static str, rule: &'static str, detail: impl Into<String>) -> ValidationResult {
    ValidationResult::Fail {
        rule_id,
        rule,
        detail: detail.into(),
    }
}

fn validate_name(name: &str) -> ValidationResult {
    const RULE: &str = "tool names are lowercase kebab-case and at most 64 characters";
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.as_bytes()[0].is_ascii_lowercase()
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        pass(RULE_NAME_FORMAT, RULE)
    } else {
        fail(RULE_NAME_FORMAT, RULE, format!("`{name}` is not a valid tool name"))
    }
}

fn validate_version(version: &str) -> ValidationResult {
    const RULE: &str = "tool versions are exact SemVer versions";
    match semver::Version::parse(version) {
        Ok(_) => pass(RULE_VERSION_SEMVER, RULE),
        Err(err) => fail(
            RULE_VERSION_SEMVER,
            RULE,
            format!("`{version}` is not an exact SemVer version: {err}"),
        ),
    }
}

fn validate_source(source: &ToolSource) -> ValidationResult {
    const RULE: &str = "tool sources are absolute paths, file:// URIs, or https:// URIs";
    let valid = match source {
        ToolSource::LocalPath(path) => path.is_absolute() || path_looks_windows_absolute(path),
        ToolSource::FileUri(uri) => {
            uri.strip_prefix("file://").is_some_and(|rest| !rest.is_empty())
        }
        ToolSource::HttpsUri(uri) => {
            uri.strip_prefix("https://").is_some_and(|rest| !rest.is_empty())
        }
    };
    if valid {
        pass(RULE_SOURCE_SUPPORTED, RULE)
    } else {
        fail(
            RULE_SOURCE_SUPPORTED,
            RULE,
            format!("`{}` is not a supported source", source.to_wire_string()),
        )
    }
}

fn validate_sha256(sha256: Option<&str>) -> ValidationResult {
    const RULE: &str = "optional sha256 pins are 64 lowercase hexadecimal characters";
    let Some(value) = sha256 else {
        return pass(RULE_SHA256_FORMAT, RULE);
    };
    if value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        pass(RULE_SHA256_FORMAT, RULE)
    } else {
        fail(RULE_SHA256_FORMAT, RULE, "`sha256` must be exactly 64 lowercase hex characters")
    }
}

fn validate_permission_paths(read: &[String], write: &[String]) -> ValidationResult {
    const RULE: &str = "permission paths are absolute or start with $PROJECT_DIR/$CAPABILITY_DIR, with no glob or parent segments";
    let failures: Vec<String> = read
        .iter()
        .map(|entry| ("read", entry))
        .chain(write.iter().map(|entry| ("write", entry)))
        .filter_map(|(kind, entry)| {
            permission_path_form_error(entry).map(|err| format!("{kind}: {err}"))
        })
        .collect();

    if failures.is_empty() {
        pass(RULE_PERMISSION_PATH_FORM, RULE)
    } else {
        fail(RULE_PERMISSION_PATH_FORM, RULE, failures.join("; "))
    }
}

fn validate_lifecycle_writes(write: &[String]) -> ValidationResult {
    const RULE: &str = "tool write permissions do not target Specify lifecycle state";
    let failures: Vec<String> = write
        .iter()
        .filter(|entry| targets_lifecycle_state(entry))
        .map(|entry| format!("write path `{entry}` targets `.specify` lifecycle state"))
        .collect();
    if failures.is_empty() {
        pass(RULE_LIFECYCLE_WRITE_DENIED, RULE)
    } else {
        fail(RULE_LIFECYCLE_WRITE_DENIED, RULE, failures.join("; "))
    }
}

fn validate_capability_dir_scope(
    scope: &ToolScope, read: &[String], write: &[String],
) -> ValidationResult {
    const RULE: &str = "$CAPABILITY_DIR is only available to capability-scope tools";
    if matches!(scope, ToolScope::Capability { .. }) {
        return pass(RULE_CAPABILITY_DIR_SCOPE, RULE);
    }

    let failures: Vec<String> = read
        .iter()
        .chain(write)
        .filter(|entry| entry.contains("$CAPABILITY_DIR"))
        .map(|entry| format!("project-scope permission `{entry}` references $CAPABILITY_DIR"))
        .collect();
    if failures.is_empty() {
        pass(RULE_CAPABILITY_DIR_SCOPE, RULE)
    } else {
        fail(RULE_CAPABILITY_DIR_SCOPE, RULE, failures.join("; "))
    }
}

fn validate_unique_names(tools: &[Tool]) -> ValidationResult {
    const RULE: &str = "tool names are unique within a single declaration site";
    let mut seen: HashSet<&str> = HashSet::new();
    let mut duplicates: Vec<&str> = Vec::new();
    for tool in tools {
        if !seen.insert(tool.name.as_str()) && !duplicates.contains(&tool.name.as_str()) {
            duplicates.push(tool.name.as_str());
        }
    }

    if duplicates.is_empty() {
        pass(RULE_NAME_UNIQUE, RULE)
    } else {
        fail(RULE_NAME_UNIQUE, RULE, format!("duplicate tool name(s): {}", duplicates.join(", ")))
    }
}

fn permission_path_form_error(value: &str) -> Option<String> {
    if value.is_empty() {
        return Some("permission path must not be empty".to_string());
    }
    if has_glob_char(value) {
        return Some(format!("`{value}` contains glob metacharacters"));
    }
    if has_parent_segment(value) {
        return Some(format!("`{value}` contains a `..` segment"));
    }
    if has_unsupported_variable(value) {
        return Some(format!("`{value}` contains an unsupported variable"));
    }
    if is_project_dir_path(value)
        || is_capability_dir_path(value)
        || Path::new(value).is_absolute()
        || looks_like_windows_absolute_str(value)
    {
        return None;
    }
    Some(format!("`{value}` must be absolute or start with $PROJECT_DIR or $CAPABILITY_DIR"))
}

fn has_glob_char(value: &str) -> bool {
    value.bytes().any(|b| matches!(b, b'*' | b'?' | b'[' | b']' | b'{' | b'}'))
}

fn has_parent_segment(value: &str) -> bool {
    value.split(['/', '\\']).any(|segment| segment == "..")
}

fn has_unsupported_variable(value: &str) -> bool {
    value.contains('$') && !is_project_dir_path(value) && !is_capability_dir_path(value)
}

fn is_project_dir_path(value: &str) -> bool {
    value == "$PROJECT_DIR"
        || value.starts_with("$PROJECT_DIR/")
        || value.starts_with("$PROJECT_DIR\\")
}

fn is_capability_dir_path(value: &str) -> bool {
    value == "$CAPABILITY_DIR"
        || value.starts_with("$CAPABILITY_DIR/")
        || value.starts_with("$CAPABILITY_DIR\\")
}

fn targets_lifecycle_state(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    let Some(specify_index) = normalized.find(".specify") else {
        return false;
    };
    let after = &normalized[specify_index..];
    after == ".specify" || after.starts_with(".specify/")
}

fn path_looks_windows_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(looks_like_windows_absolute_str)
}

fn looks_like_windows_absolute_str(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::manifest::{ToolPermissions, ToolSource};

    fn project_scope() -> ToolScope {
        ToolScope::Project {
            project_name: "demo".to_string(),
        }
    }

    fn valid_tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            source: ToolSource::HttpsUri("https://example.com/tool.wasm".to_string()),
            sha256: Some(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            ),
            permissions: ToolPermissions {
                read: vec!["$PROJECT_DIR/contracts".to_string()],
                write: vec!["$PROJECT_DIR/generated".to_string()],
            },
        }
    }

    fn fail_rule_ids(results: &[ValidationResult]) -> Vec<&'static str> {
        results
            .iter()
            .filter_map(|result| match result {
                ValidationResult::Fail { rule_id, .. } => Some(*rule_id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn tool_validate_structure_reports_all_chunk_one_rule_ids() {
        let tool = Tool {
            name: "BadName".to_string(),
            version: "not-semver".to_string(),
            source: ToolSource::HttpsUri("oci://registry/tool.wasm".to_string()),
            sha256: Some("ABC".to_string()),
            permissions: ToolPermissions {
                read: vec![
                    "relative/../*.txt".to_string(),
                    "$CAPABILITY_DIR/templates".to_string(),
                ],
                write: vec!["$PROJECT_DIR/.specify/project.yaml".to_string()],
            },
        };

        let results = tool.validate_structure(&project_scope());
        let ids = fail_rule_ids(&results);
        assert!(ids.contains(&RULE_NAME_FORMAT));
        assert!(ids.contains(&RULE_VERSION_SEMVER));
        assert!(ids.contains(&RULE_SOURCE_SUPPORTED));
        assert!(ids.contains(&RULE_SHA256_FORMAT));
        assert!(ids.contains(&RULE_PERMISSION_PATH_FORM));
        assert!(ids.contains(&RULE_LIFECYCLE_WRITE_DENIED));
        assert!(ids.contains(&RULE_CAPABILITY_DIR_SCOPE));
    }

    #[test]
    fn write_permission_to_project_root_is_valid_when_tool_needs_root_outputs() {
        let mut tool = valid_tool("contract");
        tool.permissions.write = vec!["$PROJECT_DIR".to_string()];

        let results = tool.validate_structure(&project_scope());
        assert!(results.iter().all(|result| matches!(result, ValidationResult::Pass { .. })));
    }

    #[test]
    fn manifest_validate_structure_rejects_duplicate_names() {
        let manifest = ToolManifest {
            tools: vec![valid_tool("contract"), valid_tool("contract")],
        };
        let results = manifest.validate_structure(&project_scope());
        assert!(fail_rule_ids(&results).contains(&RULE_NAME_UNIQUE));
    }

    #[test]
    fn project_scope_rejects_capability_dir_permissions() {
        let mut tool = valid_tool("contract");
        tool.permissions.read.push("$CAPABILITY_DIR/templates".to_string());

        let results = tool.validate_structure(&project_scope());
        assert!(fail_rule_ids(&results).contains(&RULE_CAPABILITY_DIR_SCOPE));
    }

    #[test]
    fn valid_tool_passes_structure_validation() {
        let results = valid_tool("contract").validate_structure(&project_scope());
        assert!(results.iter().all(|result| matches!(result, ValidationResult::Pass { .. })));
    }

    #[test]
    fn tool_schema_rejects_chunk_one_invalid_shapes() {
        let schema: serde_json::Value =
            serde_json::from_str(TOOL_JSON_SCHEMA).expect("schema parses");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let cases = [
            json!({ "tools": [{ "name": "Bad", "version": "1.0.0", "source": "/tmp/a.wasm" }] }),
            json!({ "tools": [{ "name": "bad", "version": "one", "source": "/tmp/a.wasm" }] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "relative.wasm" }] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "oci://x" }] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm", "sha256": "ABC" }] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm", "permissions": { "read": ["$PROJECT_DIR/../x"] } }] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm", "permissions": { "write": ["$PROJECT_DIR/.specify/project.yaml"] } }] }),
            json!({ "tools": [
                { "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm" },
                { "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm" }
            ] }),
            json!({ "tools": [{ "name": "bad", "version": "1.0.0", "source": "/tmp/a.wasm", "permissions": { "read": [], "exec": [] } }] }),
        ];

        for case in cases {
            assert!(!validator.is_valid(&case), "schema should reject invalid case: {case}");
        }
    }

    #[test]
    fn tool_schema_accepts_project_root_write_permission() {
        let schema: serde_json::Value =
            serde_json::from_str(TOOL_JSON_SCHEMA).expect("schema parses");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let case = json!({
            "tools": [{
                "name": "root-writer",
                "version": "1.0.0",
                "source": "/tmp/a.wasm",
                "permissions": { "write": ["$PROJECT_DIR"] }
            }]
        });

        assert!(validator.is_valid(&case), "schema should allow project-root writes: {case}");
    }
}
