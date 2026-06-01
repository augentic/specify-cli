//! Pure build-request assembly (RFC-29d M3 / D6).
//!
//! [`build_request`] is IO-free apart from existence checks against the
//! slice tree — it never writes a journal, request file, or report. It
//! resolves the singular rendered artifacts, enumerates the per-unit
//! `spec.md` files, and resolves the bound target adapter's declared
//! [`BuildInputDeclaration`]s into [`BuildArtifacts::additional`],
//! raising `target-build-input-missing` when a `required` declaration
//! names a path absent from the slice tree.

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use specify_error::{Error, Result};

use crate::adapter::BuildInputDeclaration;
use crate::slice::build::wire::{BUILD_VERSION, BuildArtifacts, BuildInputs, BuildRequest};

const PROPOSAL_ARTIFACT: &str = "proposal.md";
const DESIGN_ARTIFACT: &str = "design.md";
const TASKS_ARTIFACT: &str = "tasks.md";

/// Assemble a [`BuildRequest`] for `slice` from already-resolved
/// inputs.
///
/// `manifest_inputs` is the bound target adapter's declared build-inputs
/// list (`TargetAdapter::inputs`); `slice_tree` is the slice tree all
/// artifact paths resolve against (the request's `inputs.root`);
/// `project_dir` is the working tree the target builds into. The verb
/// (C6) resolves the target and supplies these so this stays pure.
///
/// The singular artifacts are fixed relative names; the `specs[]` are
/// the per-unit `specs/<unit>/spec.md` files found under the slice tree
/// (sorted); `additional[]` is the manifest declarations that resolve
/// against the slice tree, in declaration order.
///
/// # Errors
///
/// - [`Error::Validation`] keyed on `target-build-input-missing` (exit
///   code 2) when a `required` declaration names a path absent from the
///   slice tree.
/// - [`Error::Filesystem`] when the slice tree's `specs/` directory
///   exists but cannot be read.
pub fn build_request(
    slice: &str, manifest_inputs: &[BuildInputDeclaration], slice_tree: &Path, project_dir: &Path,
) -> Result<BuildRequest> {
    let specs = spec_paths(slice_tree)?;
    let additional = resolve_additional(manifest_inputs, slice_tree)?;
    Ok(BuildRequest {
        version: BUILD_VERSION,
        slice: slice.to_string(),
        project_dir: project_dir.to_path_buf(),
        inputs: BuildInputs {
            root: slice_tree.to_path_buf(),
            artifacts: BuildArtifacts {
                proposal: PROPOSAL_ARTIFACT.to_string(),
                design: DESIGN_ARTIFACT.to_string(),
                tasks: TASKS_ARTIFACT.to_string(),
                specs,
                additional,
            },
        },
    })
}

/// Sorted `specs/<unit>/spec.md` paths (slice-tree relative) for each
/// unit directory under `<slice_tree>/specs/` carrying a `spec.md`.
///
/// Returns an empty vector when `specs/` is missing — the request schema
/// (`specs` `minItems: 1`) catches an empty list downstream.
fn spec_paths(slice_tree: &Path) -> Result<Vec<String>> {
    let specs_dir = slice_tree.join("specs");
    if !specs_dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&specs_dir).map_err(|source| Error::Filesystem {
        op: "readdir",
        path: specs_dir.clone(),
        source,
    })?;
    let mut paths: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "readdir-entry",
            path: specs_dir.clone(),
            source,
        })?;
        let unit_dir = entry.path();
        if !unit_dir.is_dir() || !unit_dir.join("spec.md").is_file() {
            continue;
        }
        if let Some(unit) = unit_dir.file_name().and_then(OsStr::to_str) {
            paths.push(format!("specs/{unit}/spec.md"));
        }
    }
    paths.sort();
    Ok(paths)
}

/// Resolve the manifest input declarations against the slice tree.
///
/// Present declarations contribute their path (declaration order);
/// absent optional declarations are skipped; an absent `required`
/// declaration aborts.
fn resolve_additional(
    manifest_inputs: &[BuildInputDeclaration], slice_tree: &Path,
) -> Result<Vec<String>> {
    let mut additional: Vec<String> = Vec::new();
    for decl in manifest_inputs {
        if slice_tree.join(&decl.path).exists() {
            additional.push(decl.path.clone());
        } else if decl.required {
            return Err(Error::validation_failed(
                "target-build-input-missing",
                "required adapter-declared build input is present in the slice tree",
                format!("required input `{}` is absent from the slice tree", decl.path),
            ));
        }
    }
    Ok(additional)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let tree = dir.path();
        fs::create_dir_all(tree.join("specs/identity")).expect("mkdir specs");
        fs::write(tree.join("specs/identity/spec.md"), "# spec").expect("write spec");
        fs::write(tree.join("proposal.md"), "# proposal").expect("write proposal");
        fs::write(tree.join("design.md"), "# design").expect("write design");
        fs::write(tree.join("tasks.md"), "# tasks").expect("write tasks");
        dir
    }

    #[test]
    fn assembles_request_with_present_input() {
        let dir = slice_tree();
        let tree = dir.path();
        fs::write(tree.join("tokens.yaml"), "tokens: {}").expect("write tokens");
        let inputs = vec![BuildInputDeclaration {
            path: "tokens.yaml".to_string(),
            required: true,
        }];

        let req = build_request("identity-service", &inputs, tree, Path::new("/work"))
            .expect("request assembles");

        assert_eq!(req.version, BUILD_VERSION);
        assert_eq!(req.slice, "identity-service");
        assert_eq!(req.project_dir, Path::new("/work"));
        assert_eq!(req.inputs.root, tree);
        assert_eq!(req.inputs.artifacts.proposal, "proposal.md");
        assert_eq!(req.inputs.artifacts.design, "design.md");
        assert_eq!(req.inputs.artifacts.tasks, "tasks.md");
        assert_eq!(req.inputs.artifacts.specs, vec!["specs/identity/spec.md".to_string()]);
        assert_eq!(req.inputs.artifacts.additional, vec!["tokens.yaml".to_string()]);

        // The assembled request is schema-valid.
        let json = serde_json::to_string(&req).expect("serialise request");
        crate::schema::validate_build_request_json(&json).expect("assembled request validates");
    }

    #[test]
    fn missing_required_input_aborts() {
        let dir = slice_tree();
        let inputs = vec![BuildInputDeclaration {
            path: "tokens.yaml".to_string(),
            required: true,
        }];

        match build_request("identity-service", &inputs, dir.path(), Path::new("/work")) {
            Err(Error::Validation { code, .. }) => assert_eq!(code, "target-build-input-missing"),
            other => panic!("expected target-build-input-missing, got {other:?}"),
        }
    }

    #[test]
    fn missing_optional_input_is_skipped() {
        let dir = slice_tree();
        let inputs = vec![BuildInputDeclaration {
            path: "assets.yaml".to_string(),
            required: false,
        }];

        let req = build_request("identity-service", &inputs, dir.path(), Path::new("/work"))
            .expect("request assembles");
        assert!(req.inputs.artifacts.additional.is_empty());
    }
}
