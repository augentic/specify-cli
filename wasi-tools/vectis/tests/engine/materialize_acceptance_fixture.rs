//! Golden layout checks for the RFC-46 acceptance fixture (R46-S25).
//!
//! The committed tree under `tests/fixtures/acceptance/task-list/` mirrors
//! `evals/fixtures/targets/vectis/task-list/design-system/` in the
//! `augentic/specify` framework repo. Regenerate both copies together
//! after editing canonical masters.

use std::path::{Path, PathBuf};

use specify_vectis::materialize::paths::{export_layout, Platform};
use specify_vectis::validate::{ValidateArgs, ValidateMode, run};

fn acceptance_fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acceptance/task-list")
}

fn assert_layout_artifacts_exist(design_root: &Path, role: &str, kind: &str, asset_id: &str) {
    for platform in [Platform::Ios, Platform::Android] {
        let layout = export_layout(role, kind, platform, asset_id)
            .unwrap_or_else(|| panic!("export_layout({role}, {kind}, {:?}, {asset_id})", platform));
        for rel in &layout.artifacts {
            let path = design_root.join(rel);
            assert!(
                path.is_file(),
                "missing acceptance export artifact: {}",
                path.display()
            );
        }
        let pin_path = design_root.join(&layout.pin);
        assert!(
            pin_path.exists(),
            "missing acceptance export pin target: {}",
            pin_path.display()
        );
    }
}

#[test]
fn acceptance_fixture_export_layout_complete() {
    let root = acceptance_fixture_root();
    assert!(
        root.join("assets.yaml").is_file(),
        "acceptance fixture missing at {}",
        root.display()
    );

    assert_layout_artifacts_exist(&root, "app-icon", "vector", "app-icon");
    assert_layout_artifacts_exist(&root, "illustration", "vector", "empty-tasks-hero");
}

#[test]
fn acceptance_fixture_validates_cleanly() {
    let root = acceptance_fixture_root();
    let assets_path = root.join("assets.yaml");
    let envelope = run(&ValidateArgs {
        mode: ValidateMode::Assets,
        path: Some(assets_path),
    })
    .expect("validate succeeds");

    let errors = envelope
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .expect("errors array");
    assert!(
        errors.is_empty(),
        "acceptance fixture should validate cleanly: {envelope}"
    );
}
