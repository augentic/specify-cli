//! Integration tests for `specify migrate` and the
//! `specify init --check-migration` probe.
//!
//! No major-version migrators are registered, so these lock the dormant
//! command surface: the probe reports `needs-migration: false`, and
//! `specify migrate` reports no registered migrators for any version
//! window. Because the binary is pre-1.0 the version logic is driven
//! with explicit `--from` / `--to` majors rather than the binary
//! version.

use std::fs;
use std::path::Path;

use serde_json::Value;
use tempfile::TempDir;

use crate::common::{parse_json, specify_cmd};

/// Seed a throwaway project with a `.specify/project.yaml` that pins no
/// `specify_version` (so `load_for_migration` neither raises
/// `CliTooOld` nor needs a default `--from`).
fn seed_project() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join(".specify")).expect("create .specify");
    fs::write(tmp.path().join(".specify/project.yaml"), "name: proj\nadapter: omnia\n")
        .expect("seed project.yaml");
    tmp
}

fn journal_exists(root: &Path) -> bool {
    root.join(".specify/journal.jsonl").exists()
}

#[test]
fn migrate_reports_no_registered_migrators() {
    let tmp = seed_project();
    let root = tmp.path();

    let assert = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "migrate", "--from", "1.0.0", "--to", "2.0.0", "--yes"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["from"], "1.0.0");
    assert_eq!(body["to"], "2.0.0");
    assert_eq!(body["migrated"], false, "no migrators are registered");
    assert!(body["kinds"].as_array().expect("kinds array").is_empty());
    assert!(!journal_exists(root), "a no-op migrate must not append a journal event");
}

#[test]
fn check_migration_probe_reports_none() {
    let tmp = seed_project();
    let root = tmp.path();

    let assert = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "init", "--check-migration"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["version"], 1);
    assert_eq!(body["needs-migration"], false);
    assert!(body["plan"].as_array().expect("plan array").is_empty());
    assert!(body.get("to").is_some(), "the envelope always carries `to`");
    assert_eq!(body["from"], Value::Null);
}
