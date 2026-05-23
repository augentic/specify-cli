//! Integration tests for `specify target resolve`.
//!
//! Mirrors the RFC-25 target-adapter loader exposed by
//! `crates/domain/src/adapter/`. The CLI verb is a thin
//! `TargetAdapter::resolve(name, project_dir)` wrapper.

use std::fs;
use std::path::{Path, PathBuf};

mod common;
use common::{Project, parse_stdout, repo_root, specify};

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

fn stage_target_fixture(project: &Project, name: &str) {
    let src = plugin_fixtures_root().join("adapters").join("targets").join(name);
    let dst = project.root().join("adapters").join("targets").join(name);
    copy_dir_recursive(&src, &dst);
}

#[test]
fn target_resolve_local_returns_resolved_manifest() {
    let project = Project::init();
    // `Project::init()` seeds `.specify/.cache/manifests/targets/omnia/`; remove
    // it so the local probe wins for this test.
    let cached = project.root().join(".specify/.cache/manifests/targets/omnia");
    if cached.exists() {
        fs::remove_dir_all(&cached).expect("clear cached omnia");
    }
    stage_target_fixture(&project, "omnia");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "target", "resolve", "omnia"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["axis"], "targets");
    assert_eq!(actual["name"], "omnia");
    assert_eq!(actual["location"], "local");
    let ops: Vec<&str> =
        actual["operations"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    // After the `operations[]` collapse (review 1.A1), the envelope
    // derives operations from `briefs.keys()` — a BTreeMap, so order is
    // ascending kebab-name: build < merge < shape.
    assert_eq!(ops, vec!["build", "merge", "shape"]);
    let resolved = actual["resolved-path"].as_str().expect("resolved-path str");
    assert!(
        resolved.ends_with("adapters/targets/omnia"),
        "resolved-path {resolved} must end with targets/omnia"
    );
}

#[test]
fn target_resolve_strips_version_suffix() {
    // RFC-25 §CLI surface: `specify target resolve <value>` takes
    // either `<name>` or `<name>@<version>`. The `@version` part is
    // opaque metadata; the loader is keyed on the bare kebab name.
    let project = Project::init();
    stage_target_fixture(&project, "omnia");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "target", "resolve", "omnia@v1"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();

    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["name"], "omnia");
}

#[test]
fn retired_adapter_verb_rejected_by_clap() {
    // `specify adapter *` retires at 2.0 (RFC-25 §What was cut and why).
    // Clap rejects unknown verbs with exit code 2.
    let assert = specify().arg("adapter").arg("resolve").arg("omnia").assert().failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "clap must reject the retired `adapter` verb with exit 2, got {code}");
}

#[test]
fn retired_change_verb_rejected_by_clap() {
    // `specify change *` retires at 2.0 (RFC-25 §What was cut and why).
    let assert = specify().arg("change").arg("draft").arg("demo").assert().failure();
    let code = assert.get_output().status.code().expect("exit code");
    assert_eq!(code, 2, "clap must reject the retired `change` verb with exit 2, got {code}");
}
