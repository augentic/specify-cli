//! Structural validation for declared Specify WASI tools.

use std::collections::HashSet;
use std::path::Path;

use specify_error::{ValidationStatus, ValidationSummary};

use crate::manifest::{
    Tool, ToolManifest, ToolScope, ToolSource, first_party_permissions, looks_like_sha256_hex,
    looks_like_windows_absolute,
};

/// Canonical JSON Schema for the two `tools:` declaration sites.
pub const TOOL_JSON_SCHEMA: &str = include_str!("../../../schemas/tool.schema.json");

const RULE_NAME_FORMAT: &str = "tool.name-format";
const RULE_VERSION_SEMVER: &str = "tool.version-is-semver";
const RULE_SOURCE_SUPPORTED: &str = "tool.source-is-supported-uri";
const RULE_PACKAGE_FORMAT: &str = "tool.package-request-format";
const RULE_PACKAGE_NAMESPACE: &str = "tool.package-namespace-first-party";
const RULE_PACKAGE_VERSION: &str = "tool.package-version-is-exact-semver";
const RULE_PACKAGE_PERMISSIONS: &str = "tool.package-permissions-catalog";
const RULE_SHA256_FORMAT: &str = "tool.sha256-format";
const RULE_PERMISSION_PATH_FORM: &str = "tool.permission-path-form";
const RULE_LIFECYCLE_WRITE_DENIED: &str = "tool.lifecycle-state-write-denied";
const RULE_CAPABILITY_DIR_SCOPE: &str = "tool.capability-dir-out-of-scope";
const RULE_SOURCE_CAPABILITY_DIR_SCOPE: &str = "tool.source-capability-dir-out-of-scope";
const RULE_NAME_UNIQUE: &str = "tool.name-unique";

impl Tool {
    /// Validate the tool declaration against the structural rules (`name`, `version`, `source`, `sha256`, permission shape).
    #[must_use]
    pub fn validate_structure(&self, scope: &ToolScope) -> Vec<ValidationSummary> {
        let package = if let ToolSource::Package(p) = &self.source { Some(p) } else { None };

        let name_valid = !self.name.is_empty()
            && self.name.len() <= 64
            && self.name.as_bytes()[0].is_ascii_lowercase()
            && self.name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        let name_detail =
            (!name_valid).then(|| format!("`{}` is not a valid tool name", self.name));

        let version_detail = semver::Version::parse(&self.version)
            .err()
            .map(|err| format!("`{}` is not an exact SemVer version: {err}", self.version));

        let (source_detail, source_scope_detail) = validate_source(&self.source, scope);

        let package_format_detail = package.and_then(|p| {
            let valid = !p.namespace.is_empty()
                && !p.name.is_empty()
                && !p.version.is_empty()
                && p.to_wire_string().contains('@')
                && p.name_ref().contains(':');
            (!valid).then(|| format!("`{}` is not a package request", self.source.to_wire_string()))
        });

        let package_namespace_detail = package.and_then(|p| {
            (p.namespace != "specify")
                .then(|| format!("`{}` is not in the specify namespace", p.name_ref()))
        });

        let package_version_detail = package.and_then(|p| {
            if p.version.starts_with('v') {
                Some(format!("`{}` uses a leading v", p.version))
            } else {
                semver::Version::parse(&p.version)
                    .err()
                    .map(|err| format!("`{}` is not an exact SemVer version: {err}", p.version))
            }
        });

        let package_permissions_detail = package.and_then(|p| match first_party_permissions(p) {
            None => Some(format!("`{}` has no embedded permission defaults", p.name_ref())),
            Some(expected) if self.permissions == expected => None,
            Some(_) => {
                Some(format!("`{}` permissions do not match embedded defaults", p.name_ref()))
            }
        });

        let sha256_detail = self
            .sha256
            .as_deref()
            .filter(|v| !looks_like_sha256_hex(v))
            .map(|_| "`sha256` must be exactly 64 lowercase hex characters".to_string());

        vec![
            check(
                RULE_NAME_FORMAT,
                "tool names are lowercase kebab-case and at most 64 characters",
                name_detail,
            ),
            check(RULE_VERSION_SEMVER, "tool versions are exact SemVer versions", version_detail),
            check(
                RULE_SOURCE_SUPPORTED,
                "tool sources are absolute paths, file:// URIs, https:// URIs, $PROJECT_DIR/$CAPABILITY_DIR templates, or wasm package requests",
                source_detail,
            ),
            check(
                RULE_SOURCE_CAPABILITY_DIR_SCOPE,
                "$CAPABILITY_DIR in source is only available to capability-scope tools",
                source_scope_detail,
            ),
            check(
                RULE_PACKAGE_FORMAT,
                "package sources use namespace:name@version syntax",
                package_format_detail,
            ),
            check(
                RULE_PACKAGE_NAMESPACE,
                "package sources use the first-party specify namespace",
                package_namespace_detail,
            ),
            check(
                RULE_PACKAGE_VERSION,
                "package sources include an exact SemVer version without a leading v",
                package_version_detail,
            ),
            check(
                RULE_PACKAGE_PERMISSIONS,
                "first-party package tools have embedded permission defaults",
                package_permissions_detail,
            ),
            check(
                RULE_SHA256_FORMAT,
                "optional sha256 pins are 64 lowercase hexadecimal characters",
                sha256_detail,
            ),
            validate_permission_paths(&self.permissions.read, &self.permissions.write),
            validate_lifecycle_writes(&self.permissions.write),
            validate_capability_dir_scope(scope, &self.permissions.read, &self.permissions.write),
        ]
    }
}

impl ToolManifest {
    /// Validate a manifest and all of its contained tools.
    #[must_use]
    pub fn validate_structure(&self, scope: &ToolScope) -> Vec<ValidationSummary> {
        let mut results = Vec::with_capacity(1 + self.tools.len() * 12);

        let mut seen: HashSet<&str> = HashSet::new();
        let mut duplicates: Vec<&str> = Vec::new();
        for tool in &self.tools {
            if !seen.insert(tool.name.as_str()) && !duplicates.contains(&tool.name.as_str()) {
                duplicates.push(tool.name.as_str());
            }
        }
        results.push(check(
            RULE_NAME_UNIQUE,
            "tool names are unique within a single declaration site",
            (!duplicates.is_empty())
                .then(|| format!("duplicate tool name(s): {}", duplicates.join(", "))),
        ));

        for tool in &self.tools {
            results.extend(tool.validate_structure(scope));
        }
        results
    }
}

fn check(rule_id: &'static str, rule: &'static str, detail: Option<String>) -> ValidationSummary {
    let status = if detail.is_none() { ValidationStatus::Pass } else { ValidationStatus::Fail };
    ValidationSummary {
        status,
        rule_id: rule_id.to_string(),
        rule: rule.to_string(),
        detail,
    }
}

fn validate_source(source: &ToolSource, scope: &ToolScope) -> (Option<String>, Option<String>) {
    let valid = match source {
        ToolSource::LocalPath(path) => {
            path.is_absolute() || path.to_str().is_some_and(looks_like_windows_absolute)
        }
        ToolSource::FileUri(uri) => {
            uri.strip_prefix("file://").is_some_and(|rest| !rest.is_empty())
        }
        ToolSource::HttpsUri(uri) => {
            uri.strip_prefix("https://").is_some_and(|rest| !rest.is_empty())
        }
        ToolSource::Package(p) => !p.name_ref().is_empty(),
        ToolSource::TemplatePath(t) => {
            t.starts_with("$PROJECT_DIR") || t.starts_with("$CAPABILITY_DIR")
        }
    };
    let detail =
        (!valid).then(|| format!("`{}` is not a supported source", source.to_wire_string()));
    let scope_detail = if let ToolSource::TemplatePath(t) = source {
        (t.contains("$CAPABILITY_DIR") && !matches!(scope, ToolScope::Capability { .. }))
            .then(|| "project-scope source references $CAPABILITY_DIR".to_string())
    } else {
        None
    };
    (detail, scope_detail)
}

fn validate_permission_paths(read: &[String], write: &[String]) -> ValidationSummary {
    const RULE: &str = "permission paths are absolute or start with $PROJECT_DIR/$CAPABILITY_DIR, with no glob or parent segments";
    let failures: Vec<String> = read
        .iter()
        .map(|entry| ("read", entry))
        .chain(write.iter().map(|entry| ("write", entry)))
        .filter_map(|(kind, entry)| {
            permission_path_form_error(entry).map(|err| format!("{kind}: {err}"))
        })
        .collect();
    check(RULE_PERMISSION_PATH_FORM, RULE, (!failures.is_empty()).then(|| failures.join("; ")))
}

fn validate_lifecycle_writes(write: &[String]) -> ValidationSummary {
    const RULE: &str = "tool write permissions do not target Specify lifecycle state";
    let failures: Vec<String> = write
        .iter()
        .filter(|entry| targets_lifecycle_state(entry))
        .map(|entry| format!("write path `{entry}` targets `.specify` lifecycle state"))
        .collect();
    check(RULE_LIFECYCLE_WRITE_DENIED, RULE, (!failures.is_empty()).then(|| failures.join("; ")))
}

fn validate_capability_dir_scope(
    scope: &ToolScope, read: &[String], write: &[String],
) -> ValidationSummary {
    const RULE: &str = "$CAPABILITY_DIR is only available to capability-scope tools";
    let failures: Vec<String> = if matches!(scope, ToolScope::Capability { .. }) {
        Vec::new()
    } else {
        read.iter()
            .chain(write)
            .filter(|entry| entry.contains("$CAPABILITY_DIR"))
            .map(|entry| format!("project-scope permission `{entry}` references $CAPABILITY_DIR"))
            .collect()
    };
    check(RULE_CAPABILITY_DIR_SCOPE, RULE, (!failures.is_empty()).then(|| failures.join("; ")))
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
        || looks_like_windows_absolute(value)
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::manifest::{PackageRequest, ToolPermissions, ToolSource};

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

    fn fail_rule_ids(results: &[ValidationSummary]) -> Vec<&str> {
        results
            .iter()
            .filter(|s| s.status == ValidationStatus::Fail)
            .map(|s| s.rule_id.as_str())
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
    fn package_tool_validation_reports_package_rule_ids() {
        let tool = Tool {
            name: "contract".to_string(),
            version: "v1".to_string(),
            source: ToolSource::Package(PackageRequest {
                namespace: "other".to_string(),
                name: "contract".to_string(),
                version: "v1".to_string(),
            }),
            sha256: None,
            permissions: ToolPermissions::default(),
        };

        let results = tool.validate_structure(&project_scope());
        let ids = fail_rule_ids(&results);
        assert!(ids.contains(&RULE_VERSION_SEMVER));
        assert!(ids.contains(&RULE_PACKAGE_NAMESPACE));
        assert!(ids.contains(&RULE_PACKAGE_VERSION));
        assert!(ids.contains(&RULE_PACKAGE_PERMISSIONS));
    }

    #[test]
    fn scalar_contract_package_passes_with_embedded_permissions() {
        let manifest: ToolManifest =
            serde_saphyr::from_str("tools:\n  - \"specify:contract@1.2.3\"\n")
                .expect("parse scalar package");
        let results = manifest.validate_structure(&project_scope());
        assert!(results.iter().all(|s| s.status == ValidationStatus::Pass));
    }

    #[test]
    fn write_permission_to_project_root_is_valid_when_tool_needs_root_outputs() {
        let mut tool = valid_tool("contract");
        tool.permissions.write = vec!["$PROJECT_DIR".to_string()];

        let results = tool.validate_structure(&project_scope());
        assert!(results.iter().all(|s| s.status == ValidationStatus::Pass));
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
        assert!(results.iter().all(|s| s.status == ValidationStatus::Pass));
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
            json!({ "tools": ["other:bad@1.0.0"] }),
            json!({ "tools": ["specify:bad@v1.0.0"] }),
            json!({ "tools": ["specify:bad@latest"] }),
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

    #[test]
    fn tool_schema_accepts_scalar_and_object_package_requests() {
        let schema: serde_json::Value =
            serde_json::from_str(TOOL_JSON_SCHEMA).expect("schema parses");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        for case in [
            json!({ "tools": ["specify:contract@1.2.3"] }),
            json!({ "tools": [{ "name": "contract", "version": "1.2.3", "source": "specify:contract@1.2.3" }] }),
        ] {
            assert!(validator.is_valid(&case), "schema should accept package case: {case}");
        }
    }

    #[test]
    fn tool_schema_accepts_template_path_sources() {
        let schema: serde_json::Value =
            serde_json::from_str(TOOL_JSON_SCHEMA).expect("schema parses");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        for case in [
            json!({ "tools": [{ "name": "vectis", "version": "0.3.0", "source": "$PROJECT_DIR/../cli/target/vectis.wasm" }] }),
            json!({ "tools": [{ "name": "vectis", "version": "0.3.0", "source": "$PROJECT_DIR/tools/vectis.wasm" }] }),
            json!({ "tools": [{ "name": "vectis", "version": "0.3.0", "source": "$CAPABILITY_DIR/bin/vectis.wasm" }] }),
        ] {
            assert!(validator.is_valid(&case), "schema should accept template source: {case}");
        }
    }

    #[test]
    fn template_source_passes_structure_validation() {
        let tool = Tool {
            name: "vectis".to_string(),
            version: "0.3.0".to_string(),
            source: ToolSource::TemplatePath("$PROJECT_DIR/../cli/target/vectis.wasm".to_string()),
            sha256: None,
            permissions: ToolPermissions {
                read: vec!["$PROJECT_DIR".to_string()],
                write: Vec::new(),
            },
        };
        let results = tool.validate_structure(&project_scope());
        assert!(results.iter().all(|s| s.status == ValidationStatus::Pass), "{results:?}");
    }

    #[test]
    fn template_source_capability_dir_rejected_in_project_scope() {
        let tool = Tool {
            name: "vectis".to_string(),
            version: "0.3.0".to_string(),
            source: ToolSource::TemplatePath("$CAPABILITY_DIR/bin/vectis.wasm".to_string()),
            sha256: None,
            permissions: ToolPermissions::default(),
        };
        let results = tool.validate_structure(&project_scope());
        assert!(fail_rule_ids(&results).contains(&RULE_SOURCE_CAPABILITY_DIR_SCOPE));
    }
}
