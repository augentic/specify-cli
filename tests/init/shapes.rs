//! Acceptance matrix for the four `specify init` shapes (RFC-30 Wave E
//! item 6): `greenfield`, `brownfield`, `workspace`, and `migrated`.
//!
//! Each test drives the real `specify` binary over a throwaway tempdir
//! and asserts the on-disk + JSON-envelope contract for one shape:
//!
//! - `greenfield` — a fresh `specify init <adapter>` over an empty dir
//!   scaffolds `.specify/` and pins the current `specify_version`.
//! - `brownfield` — `specify init --upgrade` over a populated regular
//!   project bumps only the pin, keeps operator artifacts byte-stable,
//!   and re-runs as a no-op.
//! - `workspace` — the same re-entry over a populated workspace,
//!   with the `workspace: true` discriminator and `registry.yaml` preserved.
//! - `migrated` — the new end-to-end: `specify migrate` transforms a v1
//!   tree into the golden v2 tree, then `specify init --upgrade`
//!   re-enters the migrated artifact set.
//!
//! The `brownfield` / `workspace` headline invariants are also covered with an
//! exhaustive byte-level write-set diff by Change E's
//! `init_upgrade_bumps_only_version_and_preserves_artifacts` and
//! `init_upgrade_preserves_workspace_and_registry` in `tests/init.rs`; the
//! versions here keep all four shapes co-located as one readable matrix.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use specify_workflow::config::ProjectConfig;
use tempfile::tempdir;

use crate::common::{copy_dir, omnia_schema_dir, parse_json, repo_root, specify_cmd};

/// Version this binary stamps into `specify_version` (the `specify`
/// crate and this test crate share the workspace version).
const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---- greenfield ----

#[test]
fn greenfield() {
    let tmp = tempdir().unwrap();
    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "init"])
        .arg(omnia_schema_dir())
        .args(["--name", "greenfield-proj"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["adapter-name"], "omnia");
    assert_eq!(body["specify-version"], BINARY_VERSION);
    assert_eq!(body["specify-version-changed"], true);

    // Fresh init scaffolds the canonical `.specify/` skeleton.
    for dir in
        [".specify", ".specify/slices", ".specify/specs", ".specify/archive", ".specify/.cache"]
    {
        assert!(tmp.path().join(dir).is_dir(), "greenfield must scaffold {dir}");
    }

    let cfg = load_cfg(tmp.path());
    assert_eq!(cfg.specify_version.as_deref(), Some(BINARY_VERSION));
    // `adapter:` persists the resolved source value (the fixture URI),
    // while the JSON envelope above carries the resolved name `omnia`.
    assert!(
        cfg.adapter.as_deref().is_some_and(|value| value.contains("omnia")),
        "greenfield must persist the omnia adapter, got {:?}",
        cfg.adapter,
    );
    assert!(!cfg.workspace, "greenfield must not write the workspace discriminator");
}

// ---- brownfield ----

#[test]
fn brownfield() {
    // Concise matrix view of the regular re-entry upgrade. Exhaustive
    // write-set coverage: tests/init.rs::
    // init_upgrade_bumps_only_version_and_preserves_artifacts (Change E).
    let tmp = tempdir().unwrap();
    let specify = tmp.path().join(".specify");
    fs::create_dir_all(specify.join("slices/my-slice")).unwrap();
    fs::write(
        specify.join("project.yaml"),
        "name: brownfield\nadapter: omnia\nspecify_version: 0.2.0\nrules:\n  specs: specs.md\n",
    )
    .unwrap();
    fs::write(specify.join("slices/my-slice/spec.md"), "# operator slice\n").unwrap();
    fs::write(tmp.path().join("AGENTS.md"), "# operator AGENTS.md\n").unwrap();

    let before = snapshot(tmp.path());
    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["specify-version"], BINARY_VERSION);
    assert_eq!(body["specify-version-changed"], true);
    assert_eq!(body["adapter-name"], "omnia");

    assert_only_project_yaml_changed(&before, &snapshot(tmp.path()));
    let cfg = load_cfg(tmp.path());
    assert_eq!(cfg.specify_version.as_deref(), Some(BINARY_VERSION));
    assert_eq!(cfg.adapter.as_deref(), Some("omnia"));

    assert_second_run_is_noop(tmp.path());
}

// ---- workspace ----

#[test]
fn workspace() {
    // Concise matrix view of the workspace re-entry upgrade. Exhaustive
    // coverage: tests/init.rs::init_upgrade_preserves_workspace_and_registry
    // (Change E).
    let tmp = tempdir().unwrap();
    let specify = tmp.path().join(".specify");
    fs::create_dir_all(&specify).unwrap();
    fs::write(
        specify.join("project.yaml"),
        "name: platform-workspace\nspecify_version: 0.2.0\nworkspace: true\n",
    )
    .unwrap();
    fs::write(tmp.path().join("registry.yaml"), "version: 1\nprojects: []\n").unwrap();
    fs::write(tmp.path().join("AGENTS.md"), "# workspace sentinel\n").unwrap();

    let before = snapshot(tmp.path());
    let assert = specify_cmd()
        .current_dir(tmp.path())
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["specify-version"], BINARY_VERSION);
    assert_eq!(body["specify-version-changed"], true);
    assert_eq!(body["adapter-name"], "workspace");

    assert_only_project_yaml_changed(&before, &snapshot(tmp.path()));
    let cfg = load_cfg(tmp.path());
    assert!(cfg.workspace, "workspace discriminator must survive the upgrade");
    assert!(cfg.adapter.is_none(), "workspace upgrade must not synthesise an adapter");
    assert_eq!(cfg.specify_version.as_deref(), Some(BINARY_VERSION));

    assert_second_run_is_noop(tmp.path());
}

// ---- migrated (end-to-end: migrate -> init --upgrade) ----

#[test]
fn migrated() {
    // === A. migrate v1 -> v2 (real cross-major transform) ===
    //
    // Cross-major migration is driven with explicit `--from` / `--to`
    // because the binary is pre-1.0: a same-major `resolve` is empty at
    // major 0, so only an explicit major-2 target fires the `v1-to-v2`
    // hop.
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    copy_dir(&migrate_fixture("before"), root);
    fs::create_dir_all(root.join(".specify")).unwrap();
    fs::write(root.join(".specify/project.yaml"), "name: legacy\nadapter: code-typescript\n")
        .unwrap();

    let assert = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "migrate", "--from", "1.0.0", "--to", "2.0.0", "--yes"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["migrated"], true);
    let kind = &body["kinds"].as_array().expect("kinds array")[0];
    assert_eq!(kind["kind"], "v1-to-v2");
    assert_eq!(kind["status"], "applied");
    assert_eq!(kind["files-rewritten"], 6, "2 adapter manifests + 2 notes + discovery + plan");
    assert_eq!(kind["files-moved"], 5, "2 source briefs + 3 target briefs");
    let removed =
        kind["files"].as_array().unwrap().iter().filter(|f| f["change"] == "removed").count();
    assert_eq!(removed, 2, "2 monolithic adapter.yaml files removed");

    // The migrated tree is byte-identical to the golden `after/`.
    let produced = snapshot_no_specify(root);
    let expected = snapshot_no_specify(&migrate_fixture("after"));
    assert_eq!(
        produced.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "migrated tree file set must match after/",
    );
    for (rel, bytes) in &expected {
        assert_eq!(produced.get(rel), Some(bytes), "byte mismatch for {}", rel.display());
    }

    // The pin was bumped to the migration target and the apply journaled
    // its counts.
    assert_eq!(load_cfg(root).specify_version.as_deref(), Some("2.0.0"));
    let events = journal_events(root);
    assert_eq!(events.len(), 1, "exactly one migration.applied event");
    assert_eq!(events[0]["event"], "migration.applied");
    assert_eq!(events[0]["payload"]["kind"], "v1-to-v2");
    assert_eq!(events[0]["payload"]["files-rewritten"], 6);
    assert_eq!(events[0]["payload"]["files-moved"], 5);

    // === B. init --upgrade on the freshly-migrated tree ===
    //
    // The pre-1.0 test binary is older than the 2.0.0 floor migrate just
    // wrote, so the re-entry upgrade hits the version floor and refuses
    // (exit 3, `specify-version-too-old`). A production >=2.x binary
    // would instead bump to its own version and no-op — that bump path
    // is exercised green in section C below. This arm locks the honest
    // behavior of `migrate` followed by `init --upgrade` on the same
    // tree under a stale binary.
    let floor = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .failure();
    assert_eq!(floor.get_output().status.code(), Some(3), "stale binary maps to exit 3");
    assert_eq!(parse_json(&floor.get_output().stderr)["error"], "specify-version-too-old");

    // === C. init --upgrade re-enters the migrated artifact set ===
    //
    // Re-stage the migrated `after/` artifacts under a same-line older
    // pin so the re-entry upgrade the migrated shape promises runs green:
    // it bumps the pin to the binary version, leaves every migrated
    // artifact byte-stable, and a second run is a no-op. This is the
    // behavior a >=2.x binary exhibits over a freshly-migrated project.
    let tmp2 = tempdir().unwrap();
    let root2 = tmp2.path();
    copy_dir(&migrate_fixture("after"), root2);
    fs::create_dir_all(root2.join(".specify")).unwrap();
    fs::write(
        root2.join(".specify/project.yaml"),
        "name: migrated\nadapter: omnia\nspecify_version: 0.2.0\n",
    )
    .unwrap();

    let before = snapshot(root2);
    let assert = specify_cmd()
        .current_dir(root2)
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    let body = parse_json(&assert.get_output().stdout);
    assert_eq!(body["specify-version"], BINARY_VERSION);
    assert_eq!(body["specify-version-changed"], true);
    assert_eq!(body["adapter-name"], "omnia");

    assert_only_project_yaml_changed(&before, &snapshot(root2));
    assert_eq!(load_cfg(root2).specify_version.as_deref(), Some(BINARY_VERSION));

    assert_second_run_is_noop(root2);
}

// ---- helpers ----

/// Root of the checked-in B2 migrate fixture (`before/` + `after/`).
fn migrate_fixture(leaf: &str) -> PathBuf {
    repo_root().join("crates/workflow/tests/migrate/v1-to-v2").join(leaf)
}

/// Parse `.specify/project.yaml` under `root` into a [`ProjectConfig`].
fn load_cfg(root: &Path) -> ProjectConfig {
    let text = fs::read_to_string(root.join(".specify/project.yaml")).expect("read project.yaml");
    serde_saphyr::from_str(&text).expect("parse project.yaml")
}

/// Journal events appended to `<root>/.specify/journal.jsonl`, or an
/// empty vector when the file is absent.
fn journal_events(root: &Path) -> Vec<Value> {
    let raw = fs::read_to_string(root.join(".specify/journal.jsonl")).unwrap_or_default();
    raw.lines().filter(|l| !l.is_empty()).map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Snapshot every regular file under `root` as `relative-path -> bytes`.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    collect(root, false)
}

/// Snapshot every regular file under `root`, skipping the `.specify/`
/// state tree the migrator owns.
fn snapshot_no_specify(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    collect(root, true)
}

fn collect(root: &Path, skip_specify: bool) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, skip_specify: bool, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let rel = path.strip_prefix(root).expect("strip prefix").to_path_buf();
            if skip_specify && rel.starts_with(".specify") {
                continue;
            }
            if entry.file_type().expect("file_type").is_dir() {
                walk(root, &path, skip_specify, out);
            } else {
                out.insert(rel, fs::read(&path).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, skip_specify, &mut out);
    out
}

/// Assert the only path that differs between two tree snapshots is
/// `.specify/project.yaml` — the closed write set of `init --upgrade`.
fn assert_only_project_yaml_changed(
    before: &BTreeMap<PathBuf, Vec<u8>>, after: &BTreeMap<PathBuf, Vec<u8>>,
) {
    let project_yaml = PathBuf::from(".specify/project.yaml");
    let before_keys: Vec<_> = before.keys().filter(|k| **k != project_yaml).collect();
    let after_keys: Vec<_> = after.keys().filter(|k| **k != project_yaml).collect();
    assert_eq!(before_keys, after_keys, "upgrade must not add or remove files");
    for key in before_keys {
        assert_eq!(before[key], after[key], "file {} must be byte-identical", key.display());
    }
}

/// Run a second `init --upgrade` over `root` and assert it is a
/// byte-stable no-op (`specify-version-changed: false`, tree unchanged).
fn assert_second_run_is_noop(root: &Path) {
    let before = snapshot(root);
    let assert = specify_cmd()
        .current_dir(root)
        .args(["--format", "json", "init", "--upgrade"])
        .assert()
        .success();
    assert_eq!(
        parse_json(&assert.get_output().stdout)["specify-version-changed"],
        false,
        "second --upgrade must be a no-op",
    );
    assert_eq!(snapshot(root), before, "second --upgrade must leave the tree byte-identical");
}
