//! Integration tests for `specrun migrate` and the
//! `specrun init --check-migration` probe (RFC-30 §D3, Wave B items 2,
//! 5).
//!
//! The migrator's golden behaviour is covered in the workflow crate
//! (`crates/workflow/tests/migrate.rs`); these tests drive the CLI
//! command surface end-to-end over the same B2 fixture: the
//! `migration.applied` journal event, the `--dry-run` no-write
//! contract, the `project.yaml` version bump, and the probe JSON
//! shape. Because the binary is pre-1.0 the version logic is driven
//! with explicit `--from` / `--to` majors rather than the binary
//! version.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

mod common;
use common::{copy_dir, parse_json, repo_root, specrun};

/// Root of the checked-in B2 fixture (`before/` + `after/`).
fn fixture_dir(leaf: &str) -> PathBuf {
    repo_root().join("crates/workflow/tests/migrate/v1-to-v2").join(leaf)
}

/// Stage `before/` into a tempdir and seed a `.specify/project.yaml`
/// that pins no `specify_version` (so `load_for_migration` neither
/// raises `CliTooOld` nor needs a default `--from`).
fn stage_before() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture_dir("before"), tmp.path());
    fs::create_dir_all(tmp.path().join(".specify")).expect("create .specify");
    fs::write(tmp.path().join(".specify/project.yaml"), "name: legacy\nadapter: code-typescript\n")
        .expect("seed project.yaml");
    tmp
}

/// Collect every file under `root` as `relative-path -> bytes`,
/// skipping the `.specify/` scratch + state tree.
fn collect_files(root: &Path) -> BTreeMap<PathBuf, String> {
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if rel.starts_with(".specify") {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            walk(root, &path, files);
        } else {
            files.insert(rel, fs::read_to_string(&path).unwrap());
        }
    }
}

fn journal_lines(root: &Path) -> Vec<Value> {
    let path = root.join(".specify/journal.jsonl");
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines().filter(|l| !l.is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn applies_fixture_journals_counts() {
    let tmp = stage_before();
    let root = tmp.path();

    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "migrate", "--from", "1.0.0", "--to", "2.0.0", "--yes"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["from"], "1.0.0");
    assert_eq!(body["to"], "2.0.0");
    assert_eq!(body["dry-run"], false);
    assert_eq!(body["migrated"], true);

    let kinds = body["kinds"].as_array().expect("kinds array");
    assert_eq!(kinds.len(), 1, "one registered hop for 1 -> 2");
    let kind = &kinds[0];
    assert_eq!(kind["kind"], "v1-to-v2");
    assert_eq!(kind["status"], "applied");
    let rewritten = kind["files-rewritten"].as_u64().expect("files-rewritten");
    let moved = kind["files-moved"].as_u64().expect("files-moved");
    let removed = kind["files"]
        .as_array()
        .expect("files array")
        .iter()
        .filter(|f| f["change"] == "removed")
        .count();
    assert_eq!(rewritten, 6, "2 adapter manifests + 2 notes + discovery + plan");
    assert_eq!(moved, 5, "2 source briefs + 3 target briefs");
    assert_eq!(removed, 2, "2 monolithic adapter.yaml files removed");

    // The migrated tree matches the golden `after/` (excluding the
    // `.specify/` state tree the command owns).
    let produced = collect_files(root);
    let expected = collect_files(&fixture_dir("after"));
    assert_eq!(
        produced.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "migrated tree file set must match after/"
    );
    for (rel, bytes) in &expected {
        assert_eq!(produced.get(rel), Some(bytes), "byte mismatch for {}", rel.display());
    }

    // `project.yaml.specify_version` was bumped to the target.
    let config = fs::read_to_string(root.join(".specify/project.yaml")).unwrap();
    assert!(config.contains("specify_version: 2.0.0"), "version floor must be cleared:\n{config}");

    // Exactly one `migration.applied` event carrying the report counts.
    let events = journal_lines(root);
    assert_eq!(events.len(), 1, "one migration.applied event");
    assert_eq!(events[0]["event"], "migration.applied");
    assert_eq!(events[0]["payload"]["kind"], "v1-to-v2");
    assert_eq!(events[0]["payload"]["files-rewritten"].as_u64(), Some(rewritten));
    assert_eq!(events[0]["payload"]["files-moved"].as_u64(), Some(moved));
}

#[test]
fn migrate_dry_run_writes_nothing() {
    let tmp = stage_before();
    let root = tmp.path();
    let before = collect_files(root);

    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "migrate", "--from", "1.0.0", "--to", "2.0.0", "--dry-run"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["dry-run"], true);
    assert_eq!(body["migrated"], true, "the plan is non-empty");
    let kind = &body["kinds"].as_array().expect("kinds")[0];
    assert_eq!(kind["status"], "planned");
    assert_eq!(kind["files-rewritten"], 6);
    assert_eq!(kind["files-moved"], 5);

    assert_eq!(collect_files(root), before, "dry-run must not mutate the tree");
    assert!(
        !root.join(".specify/journal.jsonl").exists(),
        "dry-run must not append a journal event"
    );
    let config = fs::read_to_string(root.join(".specify/project.yaml")).unwrap();
    assert!(!config.contains("specify_version"), "dry-run must not bump the version");
}

#[test]
fn migrate_without_consent_refuses() {
    let tmp = stage_before();
    let root = tmp.path();

    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "migrate", "--from", "1.0.0", "--to", "2.0.0"])
        .assert()
        .failure();
    let body = parse_json(&assert.get_output().stderr);
    assert_eq!(body["error"], "migrate-consent-required");
    assert!(
        !root.join(".specify/journal.jsonl").exists(),
        "a refused migrate must not write a journal event"
    );
}

#[test]
fn check_migration_probe_v1_tree() {
    // At a pre-1.0 binary the probe is dormant (`to` is the binary
    // version), so a v1-shaped tree with no pin reports
    // `needs-migration: false`. The probe's `true` data path is
    // covered by the workflow-crate `probe` unit tests.
    let tmp = stage_before();
    let root = tmp.path();

    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "init", "--check-migration"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["version"], 1);
    assert_eq!(body["needs-migration"], false);
    assert_eq!(body["from"], Value::Null);
    assert!(body["plan"].as_array().expect("plan array").is_empty());
    assert!(body.get("to").is_some(), "the envelope always carries `to`");
}

#[test]
fn check_migration_probe_v2_tree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    copy_dir(&fixture_dir("after"), root);
    fs::create_dir_all(root.join(".specify")).expect("create .specify");
    fs::write(
        root.join(".specify/project.yaml"),
        format!("name: migrated\nadapter: omnia\nspecify_version: {}\n", env!("CARGO_PKG_VERSION")),
    )
    .expect("seed project.yaml");

    let assert = specrun()
        .current_dir(root)
        .args(["--format", "json", "init", "--check-migration"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);

    assert_eq!(body["version"], 1);
    assert_eq!(body["needs-migration"], false);
    assert_eq!(body["from"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["to"], env!("CARGO_PKG_VERSION"));
    assert!(body["plan"].as_array().expect("plan array").is_empty());
}
