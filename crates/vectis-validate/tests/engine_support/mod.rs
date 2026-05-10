//! Shared helpers for the per-mode integration tests under
//! `tests/engine_*.rs`. Each `tests/engine_*.rs` is its own binary
//! target, so individual helpers look "dead" to whichever binary
//! does not call them; silence the lint at module scope.

#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;

use serde_json::Value;
use tempfile::{NamedTempFile, TempDir};
use vectis_validate::CommandOutcome;

pub fn extract_envelope(outcome: CommandOutcome) -> Value {
    match outcome {
        CommandOutcome::Success(value) => value,
        #[allow(unreachable_patterns)]
        _ => panic!("unexpected non-success outcome"),
    }
}

pub fn errors_array(envelope: &Value) -> &[Value] {
    envelope.get("errors").and_then(Value::as_array).expect("errors array").as_slice()
}

pub fn warnings_array(envelope: &Value) -> &[Value] {
    envelope.get("warnings").and_then(Value::as_array).expect("warnings array").as_slice()
}

pub fn write_named(content: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(content.as_bytes()).expect("write fixture");
    file
}

/// Build a project tree under a fresh tempdir matching the canonical
/// Specify layout: `<root>/design-system/assets.yaml` and
/// `<root>/design-system/assets/**` for raster + vector files.
/// Returns the tempdir and the assets.yaml path.
pub fn write_assets_project(yaml: &str, raster_files: &[&str]) -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(design.join("assets/android")).expect("mkdir assets/android");
    std::fs::create_dir_all(design.join("assets/ios")).expect("mkdir assets/ios");
    let assets_path = design.join("assets.yaml");
    std::fs::write(&assets_path, yaml).expect("write assets.yaml");
    for rel in raster_files {
        let p = design.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent");
        }
        std::fs::write(&p, b"PNGSTUB").expect("write fixture file");
    }
    (tmp, assets_path)
}

/// Drop a `.specify/specs/composition.yaml` under `<project>/` so the
/// asset-validator's sibling-discovery walk picks it up.
pub fn write_specs_composition(project: &std::path::Path, yaml: &str) {
    let dir = project.join(".specify").join("specs");
    std::fs::create_dir_all(&dir).expect("mkdir .specify/specs");
    std::fs::write(dir.join("composition.yaml"), yaml).expect("write composition.yaml");
}

/// Materialise a Specify project root with `.specify/project.yaml`.
pub fn write_specify_project() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dot_specify = tmp.path().join(".specify");
    std::fs::create_dir_all(&dot_specify).expect("mkdir .specify");
    std::fs::write(dot_specify.join("project.yaml"), "name: demo\ncapability: vectis\n")
        .expect("write project.yaml");
    tmp
}
