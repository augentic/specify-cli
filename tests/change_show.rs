//! Integration tests for `specify change show` — prints the operator
//! brief at `change.md`. Pinning text + JSON shapes that skills depend
//! on.

use std::fs;
use std::path::PathBuf;

mod common;
use common::{Project, parse_stdout, specify};

fn brief_path(project: &Project) -> PathBuf {
    project.root().join("change.md")
}

fn write_brief(project: &Project, body: &str) {
    fs::write(brief_path(project), body).expect("write change.md");
}

#[test]
fn show_absent() {
    let project = Project::init();

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "show"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert!(actual.is_null(), "absent brief must serialise as null, got: {actual}");

    let text = specify().current_dir(project.root()).args(["change", "show"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    assert!(
        stdout.contains("no change brief declared"),
        "text show should say 'no change brief declared', got: {stdout:?}"
    );
    assert!(
        stdout.contains("change.md"),
        "text show should mention the brief path, got: {stdout:?}"
    );
}

#[test]
fn show_valid_text_and_json() {
    let project = Project::init();
    write_brief(
        &project,
        "---\n\
         name: traffic-modernisation\n\
         inputs:\n\
         \x20\x20- path: ./inputs/legacy/\n\
         \x20\x20\x20\x20kind: legacy-code\n\
         ---\n\
         \n\
         # Traffic modernisation\n\
         \n\
         Prose goes here.\n",
    );

    let assert = specify()
        .current_dir(project.root())
        .args(["--format", "json", "change", "show"])
        .assert()
        .success();
    let actual = parse_stdout(&assert.get_output().stdout, project.root());
    assert_eq!(actual["frontmatter"]["name"], "traffic-modernisation");
    let inputs = actual["frontmatter"]["inputs"].as_array().expect("inputs array");
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["path"], "./inputs/legacy/");
    assert_eq!(inputs[0]["kind"], "legacy-code");
    assert!(
        actual["body"].as_str().expect("body").contains("# Traffic modernisation"),
        "body should contain the heading, got: {:?}",
        actual["body"]
    );

    let text = specify().current_dir(project.root()).args(["change", "show"]).assert().success();
    let stdout = std::str::from_utf8(&text.get_output().stdout).expect("utf8");
    for fragment in ["name: traffic-modernisation", "path: ./inputs/legacy/", "kind: legacy-code"] {
        assert!(stdout.contains(fragment), "text show should mention `{fragment}`, got:\n{stdout}");
    }
}

#[test]
fn show_malformed_returns_error() {
    let project = Project::init();
    write_brief(&project, "---\nname: BadName\n---\n\nbody\n");

    let assert = specify().current_dir(project.root()).args(["change", "show"]).assert().failure();
    assert_ne!(assert.get_output().status.code(), Some(0));
    let stderr = std::str::from_utf8(&assert.get_output().stderr).expect("utf8");
    assert!(stderr.contains("change.md"), "stderr should mention change.md, got: {stderr:?}");
    assert!(
        stderr.contains("kebab-case"),
        "stderr should mention the kebab-case rule, got: {stderr:?}"
    );
}
