//! Pure adapter-directory checks for the `adapter` framework-authoring
//! tool, lifted from the host CLI's retiring `framework::check::adapter`
//! and `framework::check::tools` imperative predicates
//! (Road B framework tool).
//!
//! The tool covers the adapter-structure family: CORE-010
//! (`adapter.missing-manifest` — an adapter directory under
//! `adapters/{sources,targets}` lacks its `adapter.yaml`, a cross-fact
//! presence check the symlink-/manifest-fact passes cannot express) and
//! CORE-049 (`tools.invalid-declaration` — a target adapter's declared
//! first-party WASM tools must match a per-adapter, version-pinned
//! policy table and carry the `tools[]` object shape).
//!
//! Policy is `specify`-owned, never baked here: CORE-049's
//! `{adapter, tool, package}` policy table arrives as a parameter the
//! entrypoint reads from the rule's `config:` (forwarded by the
//! `kind: tool` evaluator). The only literals in this crate are mechanism
//! — the `adapters/{sources,targets}` axis layout, the `adapter.yaml`
//! filename, and the `specify:<name>@<version>` package-request shape.
//!
//! Carve-out posture: this crate owns its logic and depends only on
//! `serde` / `serde-saphyr` / `serde_json`, never the host diagnostics
//! crate (`main.rs` renders the wire envelope).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value as JsonValue;

/// Codex ids each check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_MISSING_MANIFEST: &str = "CORE-010";
pub const RULE_INVALID_DECLARATION: &str = "CORE-049";

const ADAPTER_FILENAME: &str = "adapter.yaml";

/// One adapter-structure violation: its codex `rule_id`, the offending
/// path (when one applies), and a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending entry.
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// One row of the CORE-049 `{adapter, tool, package}` policy table the
/// rule supplies in `config:`. `package` is the full expected
/// package-request string (e.g. `specify:contract@0.3.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedTool {
    /// Target adapter the tool must be declared under.
    pub adapter: String,
    /// Declared tool name.
    pub name: String,
    /// Version-pinned package request the declaration must equal.
    pub package: String,
}

/// CORE-010: every adapter directory under `adapters/sources` and
/// `adapters/targets` must carry an `adapter.yaml` manifest. A directory
/// with no manifest is flagged; symlinked entries are skipped.
#[must_use]
pub fn check_missing_manifest(project_dir: &Path) -> Vec<AdapterFinding> {
    let mut findings = Vec::new();
    for axis in ["sources", "targets"] {
        let axis_dir = project_dir.join("adapters").join(axis);
        findings.extend(check_axis(project_dir, &axis_dir));
    }
    findings.sort_by(|a, b| a.message.cmp(&b.message));
    findings
}

fn check_axis(project_dir: &Path, axis_dir: &Path) -> Vec<AdapterFinding> {
    let Ok(entries) = std::fs::read_dir(axis_dir) else {
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
        let path = entry.path();
        if path.join(ADAPTER_FILENAME).is_file() {
            continue;
        }
        let rel = relative_display(project_dir, &path);
        findings.push(AdapterFinding {
            rule_id: RULE_MISSING_MANIFEST,
            path: Some(rel.clone()),
            message: format!(
                "Adapter directory missing manifest: {rel} — expected {ADAPTER_FILENAME}"
            ),
        });
    }
    findings
}

/// CORE-049: validate each target adapter's first-party WASM tool
/// declarations against the `specify`-owned `expected` policy table. A
/// declared tool must exist and equal its pinned package request; the
/// `tools[]` array must carry `{ name, version }` objects.
#[must_use]
pub fn check_invalid_declaration(project_dir: &Path, expected: &[ExpectedTool]) -> Vec<AdapterFinding> {
    let mut findings = Vec::new();
    let mut cache: BTreeMap<String, Option<ResolvedAdapter>> = BTreeMap::new();
    let mut shape_reported: BTreeSet<String> = BTreeSet::new();

    for row in expected {
        let resolved = cache
            .entry(row.adapter.clone())
            .or_insert_with(|| resolve_adapter_declarations(project_dir, &row.adapter))
            .clone();

        let Some(resolved) = resolved else {
            continue;
        };

        if shape_reported.insert(row.adapter.clone()) {
            findings.extend(resolved.shape_findings.clone());
        }

        match resolved.declarations.get(&row.name) {
            None => findings.push(invalid_declaration(
                &resolved.rel,
                &format!("missing tool '{}'", row.name),
            )),
            Some(package) if package != &row.package => findings.push(invalid_declaration(
                &resolved.rel,
                &format!("'{}' package must be '{}'", row.name, row.package),
            )),
            _ => {}
        }
    }

    findings
}

#[derive(Clone)]
struct ResolvedAdapter {
    rel: String,
    declarations: BTreeMap<String, String>,
    shape_findings: Vec<AdapterFinding>,
}

fn resolve_adapter_declarations(project_dir: &Path, adapter: &str) -> Option<ResolvedAdapter> {
    let path = project_dir.join("adapters").join("targets").join(adapter).join(ADAPTER_FILENAME);
    if !path.is_file() {
        return None;
    }

    let rel = relative_display(project_dir, &path);
    let raw = std::fs::read_to_string(&path).ok()?;
    let manifest: JsonValue = serde_saphyr::from_str(&raw).ok()?;
    let tools =
        manifest.get("tools").and_then(JsonValue::as_array).cloned().unwrap_or_default();

    let mut shape_findings = Vec::new();
    let mut declarations = BTreeMap::new();

    for tool in tools {
        let Some(entry) = tool.as_object() else {
            shape_findings.push(invalid_declaration(
                &rel,
                "`tools[]` entries must be { name, version } objects under target.schema.json",
            ));
            continue;
        };

        let name = entry.get("name").and_then(JsonValue::as_str);
        let version = entry.get("version").and_then(JsonValue::as_str);
        let (Some(name), Some(version)) = (name, version) else {
            shape_findings.push(invalid_declaration(
                &rel,
                "tool object must carry string `name` and `version` fields",
            ));
            continue;
        };

        declarations.insert(name.to_string(), format!("specify:{name}@{version}"));
    }

    Some(ResolvedAdapter { rel, declarations, shape_findings })
}

fn invalid_declaration(rel: &str, detail: &str) -> AdapterFinding {
    AdapterFinding {
        rule_id: RULE_INVALID_DECLARATION,
        path: Some(rel.to_string()),
        message: format!("First-party tool declaration: {rel} — {detail}"),
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn write_adapter(root: &Path, axis: &str, name: &str, manifest: Option<&str>) -> PathBuf {
        let dir = root.join("adapters").join(axis).join(name);
        std::fs::create_dir_all(&dir).expect("adapter dir");
        if let Some(body) = manifest {
            std::fs::write(dir.join(ADAPTER_FILENAME), body).expect("write manifest");
        }
        dir
    }

    fn expected_vectis() -> Vec<ExpectedTool> {
        vec![ExpectedTool {
            adapter: "vectis".to_string(),
            name: "vectis".to_string(),
            package: "specify:vectis@0.4.0".to_string(),
        }]
    }

    #[test]
    fn missing_manifest_flags_dir_without_yaml() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_adapter(dir.path(), "targets", "orphan", None);
        write_adapter(dir.path(), "sources", "intent", Some("name: intent\n"));
        let findings = check_missing_manifest(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_MISSING_MANIFEST);
        assert_eq!(findings[0].path.as_deref(), Some("adapters/targets/orphan"));
    }

    #[test]
    fn declaration_flags_package_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_adapter(
            dir.path(),
            "targets",
            "vectis",
            Some("name: vectis\ntools:\n  - name: vectis\n    version: 0.1.0\n"),
        );
        let findings = check_invalid_declaration(dir.path(), &expected_vectis());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_INVALID_DECLARATION);
        assert!(findings[0].message.contains("package must be 'specify:vectis@0.4.0'"));
    }

    #[test]
    fn declaration_clean_when_pinned() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_adapter(
            dir.path(),
            "targets",
            "vectis",
            Some("name: vectis\ntools:\n  - name: vectis\n    version: 0.4.0\n"),
        );
        assert!(check_invalid_declaration(dir.path(), &expected_vectis()).is_empty());
    }

    #[test]
    fn declaration_flags_missing_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_adapter(dir.path(), "targets", "vectis", Some("name: vectis\n"));
        let findings = check_invalid_declaration(dir.path(), &expected_vectis());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("missing tool 'vectis'"));
    }

    #[test]
    fn declaration_skips_absent_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(check_invalid_declaration(dir.path(), &expected_vectis()).is_empty());
    }
}
