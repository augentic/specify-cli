//! Integration tests for the extraction cache fingerprint contract cache index explain surface.

use std::fs;

mod common;
use common::{Project, parse_stdout, specify_cmd};

#[test]
fn source_resolve_explain_empty_chain() {
    let project = Project::init();
    let adapter_dir = project.root().join("adapters/sources/typescript/briefs");
    fs::create_dir_all(&adapter_dir).expect("create source adapter dirs");
    fs::write(
        project.root().join("adapters/sources/typescript/adapter.yaml"),
        "name: typescript\nversion: 1\naxis: source\nexecution: agent\nbriefs:\n  survey: briefs/survey.md\n  extract: briefs/extract.md\ndescription: TypeScript test fixture.\n",
    )
    .expect("write adapter manifest");
    fs::write(adapter_dir.join("survey.md"), "---\nid: survey\ndescription: survey\n---\n")
        .expect("write survey brief");
    fs::write(adapter_dir.join("extract.md"), "---\nid: extract\ndescription: extract\n---\n")
        .expect("write extract brief");

    let assert = specify_cmd()
        .current_dir(project.root())
        .args(["--format", "json", "source", "resolve", "typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let body = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(body["adapter"], "typescript");
    assert!(body["entries"].as_array().expect("entries array").is_empty());

    let assert_text = specify_cmd()
        .current_dir(project.root())
        .args(["source", "resolve", "typescript", "--explain"])
        .arg("--project-dir")
        .arg(project.root())
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert_text.get_output().stdout);
    assert!(stdout.contains("adapter: typescript"), "text body:\n{stdout}");
    assert!(stdout.contains("(no cache writes recorded yet)"), "text body:\n{stdout}");
}
