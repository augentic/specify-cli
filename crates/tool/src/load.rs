//! Loaders and merge helpers for project and plugin tool declarations.

use std::collections::HashSet;
use std::path::Path;

use crate::error::ToolError;
use crate::manifest::{Axis, Tool, ToolManifest, ToolScope};

/// Attach a project scope to tools parsed by the binary from `ProjectConfig`.
#[must_use]
pub fn project_tools(project_name: impl Into<String>, tools: Vec<Tool>) -> Vec<(ToolScope, Tool)> {
    let scope = ToolScope::Project {
        project_name: project_name.into(),
    };
    tools.into_iter().map(|tool| (scope.clone(), tool)).collect()
}

/// Read the plugin-scope `tools.yaml` sidecar next to a resolved
/// `adapter.yaml` (per workflow §Adapter implementation shape, both
/// source and target plugins keep the `adapter.yaml` filename).
///
/// Plugins without a sidecar remain valid and return an empty list.
///
/// # Errors
///
/// Returns an error when the sidecar exists but cannot be read or parsed.
pub fn plugin_sidecar(
    plugin_dir: &Path, plugin_slug: &str, axis: Axis,
) -> Result<Vec<(ToolScope, Tool)>, ToolError> {
    let sidecar_path = plugin_dir.join("tools.yaml");
    let text = match std::fs::read_to_string(&sidecar_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(ToolError::manifest_read(sidecar_path, err)),
    };

    let manifest: ToolManifest = serde_saphyr::from_str(&text)
        .map_err(|err| ToolError::manifest_parse(sidecar_path.clone(), err))?;
    let scope = ToolScope::Plugin {
        axis,
        plugin_slug: plugin_slug.to_string(),
        capability_dir: plugin_dir.to_path_buf(),
    };
    Ok(manifest.tools.into_iter().map(|tool| (scope.clone(), tool)).collect())
}

/// Merge project and plugin declarations. Project-scope tools win on
/// name collision so operators can override plugin-shipped declarations.
#[must_use]
pub fn merge_scoped(
    project: Vec<(ToolScope, Tool)>, plugin: Vec<(ToolScope, Tool)>,
) -> (Vec<(ToolScope, Tool)>, Vec<String>) {
    let mut merged: Vec<(ToolScope, Tool)> = Vec::with_capacity(project.len() + plugin.len());
    let mut project_names: HashSet<String> = HashSet::new();
    let mut warnings: Vec<String> = Vec::new();

    for (scope, tool) in project {
        project_names.insert(tool.name.clone());
        merged.push((scope, tool));
    }

    for (scope, tool) in plugin {
        if project_names.contains(&tool.name) {
            warnings.push(tool.name);
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
    fn sidecar_empty_when_absent() {
        let tmp = tempdir().expect("tempdir");
        let loaded =
            plugin_sidecar(tmp.path(), "contracts", Axis::Target).expect("absent sidecar is valid");
        assert!(loaded.is_empty());
    }

    #[test]
    fn sidecar_rejects_wrong_shape() {
        let tmp = tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("tools.yaml"),
            "- name: bad\n  version: 1.0.0\n  source: /tmp/bad.wasm\n",
        )
        .expect("write sidecar");

        let err = plugin_sidecar(tmp.path(), "contracts", Axis::Target)
            .expect_err("array top-level shape must fail");
        assert!(
            matches!(
                err,
                ToolError::Diag {
                    code: "tool-manifest-parse",
                    ..
                }
            ),
            "{err}"
        );
    }

    #[test]
    fn load_plugin_sidecar_scopes_parsed_tools() {
        let tmp = tempdir().expect("tempdir");
        fs::write(
            tmp.path().join("tools.yaml"),
            "tools:\n  - name: contract\n    version: 1.0.0\n    source: /tmp/contract.wasm\n",
        )
        .expect("write sidecar");

        let loaded = plugin_sidecar(tmp.path(), "contracts", Axis::Target).expect("load sidecar");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1.name, "contract");
        assert!(matches!(
            &loaded[0].0,
            ToolScope::Plugin {
                axis: Axis::Target,
                plugin_slug,
                capability_dir,
            } if plugin_slug == "contracts" && capability_dir == tmp.path()
        ));
    }

    #[test]
    fn merge_project_wins_and_warns() {
        let project_scope = ToolScope::Project {
            project_name: "demo".to_string(),
        };
        let plugin_scope = ToolScope::Plugin {
            axis: Axis::Target,
            plugin_slug: "contracts".to_string(),
            capability_dir: "/cap".into(),
        };

        let project = vec![(
            project_scope,
            tool("contract", "2.0.0", ToolSource::LocalPath("/project/contract.wasm".into())),
        )];
        let plugin = vec![
            (
                plugin_scope.clone(),
                tool("contract", "1.0.0", ToolSource::LocalPath("/cap/contract.wasm".into())),
            ),
            (plugin_scope, tool("other", "1.0.0", ToolSource::LocalPath("/cap/other.wasm".into()))),
        ];

        let (merged, warnings) = merge_scoped(project, plugin);
        assert_eq!(warnings, vec!["contract".to_string()]);
        assert_eq!(
            merged.iter().map(|(_, t)| t.name.as_str()).collect::<Vec<_>>(),
            ["contract", "other"]
        );
        assert_eq!(merged[0].1.version, "2.0.0");
    }
}
