//! Golden JSON tests for `validate_slice`.
//!
//! Each test stages a fixture slice under a tempdir in the expected
//! layout (`<project>/.specify/slices/<name>/` + a copy of
//! `schemas/omnia/` under `<project>/schemas/omnia/`), runs
//! `validate_slice`, serialises the report via `serialize_report`, and
//! compares the pretty-printed JSON against a checked-in golden file.
//!
//! The goldens pin `envelope_version: 1` and the full shape of a
//! `ValidationReport` as observed by skill consumers. If you change the
//! registry, rule wording, or `rule_id`, regenerate both goldens with
//! `REGENERATE_GOLDENS=1 cargo test -p specify-domain --test goldens`.

use std::fs;
use std::path::{Path, PathBuf};

use specify_domain::capability::PipelineView;
use specify_domain::slice::SLICES_DIR_NAME;
use specify_domain::validate::{serialize_report, validate_slice};
use tempfile::TempDir;

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/crates/domain/` for this crate.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(Path::parent).expect("repo root exists").to_path_buf()
}

/// Copy a directory tree recursively. We don't pull in `fs_extra` just
/// for this so the crate's dev-dep set stays tiny.
fn copy_dir_recursive(src: &Path, dst: &Path) {
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

/// Stage a fixture + schema into a tempdir and return
/// `(tempdir_guard, slice_dir, pipeline_view)`.
fn stage_fixture(fixture_name: &str) -> (TempDir, PathBuf, PipelineView) {
    let repo = repo_root();
    let fixture_src = repo.join("crates/domain/tests/fixtures").join(fixture_name);
    let schema_src = repo.join("schemas/omnia");

    let tempdir = tempfile::tempdir().unwrap();
    let project_dir = tempdir.path().to_path_buf();

    let slice_dir = project_dir.join(".specify").join(SLICES_DIR_NAME).join(fixture_name);
    copy_dir_recursive(&fixture_src, &slice_dir);

    let schema_dst = project_dir.join("schemas").join("omnia");
    copy_dir_recursive(&schema_src, &schema_dst);

    let pipeline = PipelineView::load("omnia", &project_dir).expect("pipeline loads");

    (tempdir, slice_dir, pipeline)
}

fn golden_path(fixture_name: &str) -> PathBuf {
    repo_root().join("crates/domain/tests/fixtures").join(format!("{fixture_name}.golden.json"))
}

fn run_fixture_and_diff(fixture_name: &str, expected_passed: bool) {
    let (_guard, slice_dir, pipeline) = stage_fixture(fixture_name);
    let report = validate_slice(&slice_dir, &pipeline).expect("validate_slice ok");
    assert_eq!(
        report.passed, expected_passed,
        "report.passed mismatch for `{fixture_name}`: {report:#?}"
    );

    let value = serialize_report(&report);
    let serialised = serde_json::to_string_pretty(&value).unwrap();

    let path = golden_path(fixture_name);
    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::write(&path, format!("{serialised}\n")).unwrap();
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("missing golden {}: {err}; regenerate with REGENERATE_GOLDENS=1", path.display())
    });

    let expected_value: serde_json::Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(value, expected_value, "JSON mismatch for `{fixture_name}`. Actual:\n{serialised}");
}

#[test]
fn change_good_matches_golden() {
    run_fixture_and_diff("change-good", true);
}

#[test]
fn change_bad_matches_golden() {
    run_fixture_and_diff("change-bad", false);
}
