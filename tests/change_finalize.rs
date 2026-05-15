//! Integration tests for `specify change finalize` (RFC-9 §4C).
//!
//! Wire-level integration tests for the precondition diagnostics. The
//! happy-path classifier flow is covered by the in-process `MockProbe`
//! against the orchestrator (see `cargo test -p specify-change`). The
//! CLI tests below pin: (a) the failure-mode wire shape skill authors
//! will rely on, and (b) the on-disk archive landing when no projects
//! need probing.

use std::fs;

use serde_json::Value;
use tempfile::tempdir;

mod common;
use common::{init_hub, specify};

#[test]
fn change_help_lists_finalize() {
    let assert = specify().args(["change", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["create", "show", "finalize"] {
        assert!(
            stdout.contains(verb),
            "expected `change --help` to mention `{verb}`, got:\n{stdout}",
        );
    }
}

#[test]
fn change_finalize_help_documents_flags() {
    let assert = specify().args(["change", "finalize", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for flag in ["--clean", "--dry-run"] {
        assert!(stdout.contains(flag), "expected --help to document `{flag}`, got:\n{stdout}");
    }
    assert!(
        stdout.contains("operator-merged") || stdout.contains("gh pr merge"),
        "finalize help must explain PRs are operator-merged before finalize, got:\n{stdout}",
    );
}

#[test]
fn change_finalize_refuses_when_plan_absent() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    assert!(!tmp.path().join("plan.yaml").exists(), "test precondition: plan.yaml must be absent");

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "plan-not-found");
    let msg = value["message"].as_str().expect("message");
    assert!(msg.contains("plan.yaml"), "msg should reference plan.yaml: {msg}");
    // Diagnostic should hint at the recovery sequence — `specify
    // change draft <name>` scaffolds change.md and plan.yaml together.
    assert!(msg.contains("change draft"), "msg should hint at `change draft`, got: {msg}");
}

#[test]
fn change_finalize_refuses_on_non_terminal() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed a plan with one done and one pending entry — pending is not
    // terminal for finalize.
    fs::write(
        tmp.path().join("plan.yaml"),
        "name: foo\n\
         slices:\n\
         \x20\x20- name: a\n\
         \x20\x20\x20\x20capability: contracts@v1\n\
         \x20\x20\x20\x20status: done\n\
         \x20\x20- name: b\n\
         \x20\x20\x20\x20capability: contracts@v1\n\
         \x20\x20\x20\x20status: pending\n",
    )
    .unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(1));
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "non-terminal-entries-present");
    assert_eq!(value["exit-code"], 1);
    let msg = value["message"].as_str().expect("message string");
    assert!(msg.contains("foo"), "message names the change: {msg}");
    assert!(msg.contains('b'), "message lists the offending entry name 'b': {msg}");

    // Atomicity: plan.yaml must remain on disk on refusal.
    assert!(tmp.path().join("plan.yaml").exists(), "plan.yaml must be untouched");
}

#[test]
fn change_finalize_dry_run_archives_nothing() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    // Seed an all-terminal plan and rely on the hub-init's empty
    // registry — no per-project probes will run.
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize", "--dry-run"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["change"], "foo");
    assert_eq!(value["finalized"], true);
    assert_eq!(value["dry-run"], true, "dry-run flag must echo into JSON");
    assert!(value.get("archived").is_none(), "dry-run must not stamp archived path");
    let projects = value["projects"].as_array().expect("projects array");
    assert!(projects.is_empty(), "empty registry → empty projects, got: {projects:?}");

    assert!(tmp.path().join("plan.yaml").exists(), "dry-run must not move plan.yaml");
}

#[test]
fn change_finalize_archives_when_terminal() {
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .success();
    let value: Value = serde_json::from_slice(&assert.get_output().stdout).expect("json");
    assert_eq!(value["change"], "foo");
    assert_eq!(value["finalized"], true);
    let archived = value["archived"].as_str().expect("archived path");
    assert!(archived.contains("foo-"), "archived path must contain plan name: {archived}");
    let summary = value["summary"].as_object().expect("summary object");
    for key in
        ["merged", "unmerged", "closed", "no-branch", "branch-pattern-mismatch", "dirty", "failed"]
    {
        assert!(summary.contains_key(key), "summary missing `{key}`: {summary:?}");
    }

    assert!(!tmp.path().join("plan.yaml").exists(), "plan.yaml must be archived");
    let archive_dir = tmp.path().join(".specify/archive/plans");
    let entries: Vec<String> = fs::read_dir(&archive_dir)
        .expect("read archive dir")
        .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
        .collect();
    assert!(
        entries.iter().any(|n| n.starts_with("foo-")),
        "archive dir should contain a foo-<YYYYMMDD>.yaml: {entries:?}",
    );
}

#[test]
fn change_finalize_idempotent_after_archive() {
    // Idempotency proof: the second `finalize` invocation after the
    // archive landed produces a clear `plan-not-found` refusal — the
    // canonical "change is already finalized" signal.
    let tmp = tempdir().unwrap();
    init_hub(&tmp, "platform-hub");
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    specify().current_dir(tmp.path()).args(["change", "finalize"]).assert().success();
    assert!(!tmp.path().join("plan.yaml").exists());

    let assert = specify()
        .current_dir(tmp.path())
        .args(["--format", "json", "change", "finalize"])
        .assert()
        .failure();
    let value: Value = serde_json::from_slice(&assert.get_output().stderr).expect("json");
    assert_eq!(value["error"], "plan-not-found");
}
