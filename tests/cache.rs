//! Integration tests for the RFC-27 §D8 cache index explain surface.

use std::fs;

mod common;
use common::{Project, parse_stdout, specify};

#[test]
fn source_resolve_explain_prints_empty_fingerprint_chain() {
    let project = Project::init();
    let adapter_dir = project.root().join("adapters/sources/code-typescript/briefs");
    fs::create_dir_all(&adapter_dir).expect("create source adapter dirs");
    fs::write(
        project.root().join("adapters/sources/code-typescript/adapter.yaml"),
        "name: code-typescript\nversion: 1\naxis: source\noperations: [enumerate, extract]\nbriefs:\n  enumerate: briefs/enumerate.md\n  extract: briefs/extract.md\n",
    )
    .expect("write adapter manifest");
    fs::write(adapter_dir.join("enumerate.md"), "---\nid: enumerate\ndescription: enumerate\n---\n")
        .expect("write enumerate brief");
    fs::write(adapter_dir.join("extract.md"), "---\nid: extract\ndescription: extract\n---\n")
        .expect("write extract brief");

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "source", "resolve", "code-typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "code-typescript");
    assert!(body["entries"].as_array().expect("entries array").is_empty());

    let assert_text = specify()
        .current_dir(project.root())
        .args(["source", "resolve", "code-typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert_text.get_output().stdout);
    assert!(stdout.contains("adapter: code-typescript"), "text body:\n{stdout}");
    assert!(stdout.contains("(no cache writes recorded yet)"), "text body:\n{stdout}");
}
