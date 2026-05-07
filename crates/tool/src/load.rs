//! Loaders and merge helpers for project and capability tool declarations.

use std::collections::HashSet;
use std::path::Path;

use crate::error::ToolError;
use crate::manifest::{Tool, ToolManifest, ToolScope};

/// Warning emitted while combining declaration sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A project-scope declaration replaced a capability-scope declaration with
    /// the same name.
    ToolNameCollision {
        /// Colliding tool name.
        name: String,
    },
}

/// Attach a declaration scope to each tool in a generic list.
#[must_use]
pub fn scope_tools(scope: &ToolScope, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    tools.into_iter().map(|tool| (scope.clone(), tool)).collect()
}

/// Attach a project scope to tools parsed by the binary from `ProjectConfig`.
#[must_use]
pub fn project_tools(project_name: impl Into<String>, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    let scope = ToolScope::Project {
        project_name: project_name.into(),
    };
    scope_tools(&scope, tools)
}

/// Read the capability-scope `tools.yaml` sidecar next to a resolved
/// `capability.yaml`.
///
/// Capabilities without a sidecar remain valid and return an empty list.
///
/// # Errors
///
/// Returns an error when the sidecar exists but cannot be read or parsed.
pub fn load_capability_sidecar(
    capability_dir: &Path, capability_slug: &str,
) -> Result<Vec<(ToolScope, Tool)>, ToolError> {
    let sidecar_path = capability_dir.join("tools.yaml");
    let text = match std::fs::read_to_string(&sidecar_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(ToolError::manifest_read(sidecar_path, err)),
    };

    let manifest: ToolManifest = serde_saphyr::from_str(&text)
        .map_err(|err| ToolError::manifest_parse(sidecar_path.clone(), err))?;
    let scope = ToolScope::Capability {
        capability_slug: capability_slug.to_string(),
        capability_dir: capability_dir.to_path_buf(),
    };
    Ok(scope_tools(&scope, manifest.tools))
}

/// Merge project and capability declarations. Project-scope tools win on name
/// collision so operators can override capability-shipped declarations.
#[must_use]
pub fn merge_scoped(
    project: Vec<(ToolScope, Tool)>, capability: Vec<(ToolScope, Tool)>,
) -> (Vec<(ToolScope, Tool)>, Vec<Warning>) {
    let mut merged: Vec<(ToolScope, Tool)> = Vec::with_capacity(project.len() + capability.len());
    let mut project_names: HashSet<String> = HashSet::new();
    let mut warning_names: HashSet<String> = HashSet::new();
    let mut warnings: Vec<Warning> = Vec::new();

    for (scope, tool) in project {
        project_names.insert(tool.name.clone());
        merged.push((scope, tool));
    }

    for (scope, tool) in capability {
        if project_names.contains(&tool.name) {
            if warning_names.insert(tool.name.clone()) {
                warnings.push(Warning::ToolNameCollision { name: tool.name });
            }
            continue;
        }
        merged.push((scope, tool));
    }

    (merged, warnings)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::manifest::{ToolPermissions, ToolSource};

    fn tool(name: &str, version: &str, source: ToolSource) -> Tool {
        Tool {
            name: name.to_string(),
            version: version.to_string(),
            source,
            sha256: None,
            permissions: ToolPermissions::default(),
        }
    }

    #[test]
    fn load_capability_sidecar_returns_empty_when_absent() {
        let tmp = tempdir().expect("tempdir");
        let loaded =
            load_capability_sidecar(tmp.path(), "contracts").expect("absent sidecar is valid");
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_capability_sidecar_rejects_wrong_top_level_shape() {
        let tmp = tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("tools.yaml"),
            "- name: bad\n  version: 1.0.0\n  source: /tmp/bad.wasm\n",
        )
        .expect("write sidecar");

        let err = load_capability_sidecar(tmp.path(), "contracts")
            .expect_err("array top-level shape must fail");
        assert!(matches!(err, ToolError::ManifestParse { .. }), "{err}");
    }

    #[test]
    fn load_capability_sidecar_scopes_parsed_tools() {
        let tmp = tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("tools.yaml"),
            "tools:\n  - name: contract\n    version: 1.0.0\n    source: /tmp/contract.wasm\n",
        )
        .expect("write sidecar");

        let loaded = load_capability_sidecar(tmp.path(), "contracts").expect("load sidecar");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1.name, "contract");
        assert!(matches!(
            &loaded[0].0,
            ToolScope::Capability {
                capability_slug,
                capability_dir
            } if capability_slug == "contracts" && capability_dir == tmp.path()
        ));
    }

    #[test]
    fn merge_scoped_project_wins_and_warns_once() {
        let project_scope = ToolScope::Project {
            project_name: "demo".to_string(),
        };
        let capability_scope = ToolScope::Capability {
            capability_slug: "contracts".to_string(),
            capability_dir: "/cap".into(),
        };

        let project = vec![(
            project_scope,
            tool("contract", "2.0.0", ToolSource::LocalPath("/project/contract.wasm".into())),
        )];
        let capability = vec![
            (
                capability_scope.clone(),
                tool("contract", "1.0.0", ToolSource::LocalPath("/cap/contract.wasm".into())),
            ),
            (
                capability_scope,
                tool("other", "1.0.0", ToolSource::LocalPath("/cap/other.wasm".into())),
            ),
        ];

        let (merged, warnings) = merge_scoped(project, capability);
        assert_eq!(
            warnings,
            vec![Warning::ToolNameCollision {
                name: "contract".to_string()
            }]
        );
        assert_eq!(
            merged.iter().map(|(_, t)| t.name.as_str()).collect::<Vec<_>>(),
            ["contract", "other"]
        );
        assert_eq!(merged[0].1.version, "2.0.0");
    }
}
