//! Non-golden integration tests for `validate_change` — synthetic
//! scenarios that don't make sense to pin as static JSON.

use std::fs;
use std::path::PathBuf;

use specify_capability::{PipelineView, ValidationResult};
use specify_validate::validate_change;
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(|p| p.parent()).expect("repo root exists").to_path_buf()
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ft = entry.file_type().unwrap();
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Stage a project dir with omnia's schema but leave the change dir to
/// the caller.
fn stage_project() -> (TempDir, PathBuf) {
    let repo = repo_root();
    let schema_src = repo.join("schemas/omnia");
    let tempdir = tempfile::tempdir().unwrap();
    let project_dir = tempdir.path().to_path_buf();
    let schema_dst = project_dir.join("schemas").join("omnia");
    copy_dir_recursive(&schema_src, &schema_dst);
    (tempdir, project_dir)
}

#[test]
fn missing_artifact_produces_synth_failure() {
    let (_guard, project_dir) = stage_project();
    let change_dir = project_dir.join(".specify/changes/synth-missing");
    fs::create_dir_all(&change_dir).unwrap();
    // Deliberately leave out every artifact.

    let pipeline = PipelineView::load("omnia", &project_dir).expect("pipeline loads");
    let report = validate_change(&change_dir, &pipeline).expect("validate_change ok");

    // Every define-phase brief should have synthesised an artifact-exists
    // failure (and nothing else for that brief).
    for brief in &["proposal", "design", "tasks"] {
        let results = report
            .brief_results
            .get(*brief)
            .unwrap_or_else(|| panic!("missing entry for `{brief}`"));
        assert_eq!(results.len(), 1, "{brief} should have exactly one result");
        match &results[0] {
            ValidationResult::Fail { rule_id, detail, .. } => {
                assert!(
                    rule_id.ends_with(".artifact-exists"),
                    "unexpected rule_id for `{brief}`: {rule_id}"
                );
                assert!(detail.contains("not found"));
            }
            other => panic!("expected Fail for `{brief}`, got {other:?}"),
        }
    }
    // `specs` brief uses a glob → empty expansion also routes through the
    // artifact-missing path with key == brief_id.
    let specs = report.brief_results.get("specs").expect("specs key present");
    assert_eq!(specs.len(), 1);
    match &specs[0] {
        ValidationResult::Fail { rule_id, .. } => {
            assert_eq!(*rule_id, "specs.artifact-exists");
        }
        other => panic!("expected Fail for specs, got {other:?}"),
    }

    assert!(!report.passed);
}

#[test]
fn validate_change_reports_passed_without_panics_across_semantic_rules() {
    // Reuses the good fixture to exercise the Semantic-rules-never-called
    // invariant in situ: if any Semantic rule's `check` were invoked the
    // runner would panic (by construction) and this test would fail.
    let repo = repo_root();
    let fixture = repo.join("crates/validate/tests/fixtures/change-good");
    let (_guard, project_dir) = stage_project();
    let change_dir = project_dir.join(".specify/changes/change-good");
    copy_dir_recursive(&fixture, &change_dir);

    let pipeline = PipelineView::load("omnia", &project_dir).expect("pipeline loads");
    let report = validate_change(&change_dir, &pipeline).expect("validate_change ok");
    assert!(report.passed);

    // Confirm every Semantic rule surfaced as Deferred.
    let deferred_count: usize = report
        .brief_results
        .values()
        .flatten()
        .chain(report.cross_checks.iter())
        .filter(|r| matches!(r, ValidationResult::Deferred { .. }))
        .count();
    assert!(deferred_count >= 2, "expected at least two deferred rules");
}
