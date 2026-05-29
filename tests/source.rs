//! Integration tests for `specrun source resolve`.
//!
//! Mirrors the source-adapter loader exposed by
//! `crates/domain/src/plugin/`. The CLI verb is a thin
//! `Plugin::resolve(Axis::Source, …)` wrapper; the cases below pin
//! the wire shape skill bodies and downstream callers rely on.

use std::fs;
use std::path::{Path, PathBuf};

mod common;
use common::{Project, parse_stderr, parse_stdout, repo_root, specrun};

fn plugin_fixtures_root() -> PathBuf {
    repo_root().join("crates/domain/tests/fixtures/plugins")
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

fn stage_source_fixture(project: &Project, name: &str) {
    let src = plugin_fixtures_root().join("adapters").join("sources").join(name);
    let dst = project.root().join("adapters").join("sources").join(name);
    copy_dir_recursive(&src, &dst);
}

#[test]
fn source_resolve_local_returns_resolved_manifest() {
    let project = Project::init();
    stage_source_fixture(&project, "code-typescript");

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "resolve", "code-typescript"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["axis"], "sources");
    assert_eq!(actual["name"], "code-typescript");
    assert_eq!(actual["location"], "local");
    let operations = actual["operations"].as_array().expect("operations array");
    let ops: Vec<&str> = operations.iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(ops, vec!["enumerate", "extract"]);
    let resolved = actual["resolved-path"].as_str().expect("resolved-path str");
    assert!(
        resolved.ends_with("adapters/sources/code-typescript"),
        "resolved-path {resolved} must end with sources/code-typescript"
    );
    let briefs_dir = actual["briefs-dir"].as_str().expect("briefs-dir str");
    assert!(briefs_dir.ends_with("/briefs"), "briefs-dir {briefs_dir} must end with /briefs");
    assert!(
        briefs_dir.contains("adapters/sources/code-typescript/briefs"),
        "briefs-dir {briefs_dir} must reference the adapter's briefs directory"
    );

    // Absoluteness: the raw output used the tempdir absolute path which
    // parse_stdout substitutes to <TEMPDIR>; verify the substitution
    // happened (proving the original was absolute).
    let raw_stdout = std::str::from_utf8(&assert.get_output().stdout).unwrap();
    let raw: serde_json::Value = serde_json::from_str(raw_stdout).unwrap();
    let raw_briefs = raw["briefs-dir"].as_str().unwrap();
    assert!(Path::new(raw_briefs).is_absolute(), "briefs-dir must be absolute, got {raw_briefs}");
}

#[test]
fn source_resolve_missing_emits_not_found() {
    let project = Project::init();

    let assert = specrun()
        .current_dir(project.root())
        .args(["--format", "json", "source", "resolve", "no-such-source"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .failure();
    let stderr = parse_stderr(&assert.get_output().stderr, project.root());
    assert_eq!(stderr["error"], "adapter-not-found");
    assert_eq!(stderr["exit-code"], 1);
}
