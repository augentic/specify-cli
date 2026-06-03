//! Golden-tree tests for the `V1ToV2` migrator.
//!
//! The migrator is a library primitive (the `specify migrate` command
//! lands in a later change), so these drive it through the public
//! `specify_workflow::migrate` surface rather than the CLI. The happy
//! path copies `migrate/v1-to-v2/before/` into a tempdir, runs `plan`
//! then `apply`, and asserts the resulting file tree is byte-identical
//! to `migrate/v1-to-v2/after/`. Regenerate `after/` with
//! `REGENERATE_GOLDENS=1 cargo test -p specify-workflow --test migrate`
//! and `git diff` the result before committing.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use specify_error::Error;
use specify_model::discovery::Discovery;
use specify_workflow::adapter::{SourceAdapter, TargetAdapter};
use specify_workflow::change::Plan;
use specify_workflow::migrate::{
    MigrationKind, MigrationStatus, Migrator, V1ToV2, migrator_for, probe,
};
use tempfile::TempDir;

fn fixture_dir(leaf: &str) -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/crates/workflow/` for this crate.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/migrate/v1-to-v2").join(leaf)
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Collect every file under `root` as `relative-path -> bytes`,
/// skipping the `.specify/` scratch tree the apply harness leaves
/// behind. Directories are not recorded, so empty dirs left by moves
/// are irrelevant to the comparison.
fn collect_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
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
            files.insert(rel, fs::read(&path).unwrap());
        }
    }
}

fn stage_before() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir_recursive(&fixture_dir("before"), tmp.path());
    tmp
}

#[test]
fn matches_golden() {
    let tmp = stage_before();
    let project = tmp.path();

    let plan = V1ToV2.plan(project).expect("plan succeeds");
    let report = V1ToV2.apply(project, &plan).expect("apply succeeds");
    assert_eq!(report.status, MigrationStatus::Applied);

    let produced = collect_files(project);
    let after = fixture_dir("after");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        if after.exists() {
            fs::remove_dir_all(&after).unwrap();
        }
        for (rel, bytes) in &produced {
            let dst = after.join(rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::write(&dst, bytes).unwrap();
        }
        return;
    }

    let expected = collect_files(&after);
    let produced_keys: Vec<_> = produced.keys().collect();
    let expected_keys: Vec<_> = expected.keys().collect();
    assert_eq!(produced_keys, expected_keys, "migrated tree has unexpected file set");
    for (rel, bytes) in &produced {
        let want = expected.get(rel).unwrap();
        assert_eq!(
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(want),
            "byte mismatch for {}",
            rel.display(),
        );
    }
}

/// The migrated manifests, plan, and discovery document are valid 2.0
/// artifacts: the adapters resolve through the axis-split loader, the
/// plan loads (schema-validated, `target`-free), and discovery parses.
#[test]
fn migrated_artifacts_are_valid_v2() {
    let tmp = stage_before();
    let project = tmp.path();
    let plan = V1ToV2.plan(project).expect("plan");
    V1ToV2.apply(project, &plan).expect("apply");

    SourceAdapter::resolve("code-typescript", project).expect("source adapter resolves");
    TargetAdapter::resolve("omnia", project).expect("target adapter resolves");

    let loaded = Plan::load(&project.join("plan.yaml")).expect("plan.yaml loads as 2.0");
    assert!(
        loaded.entries.iter().all(|entry| entry.project.is_some()),
        "every slice keeps a project"
    );

    Discovery::load(&project.join("discovery.md")).expect("discovery.md parses as 2.0");
}

/// A missing move source (here a brief the adapter split would relocate)
/// aborts the apply during precondition resolution, before any staging
/// or commit, so the tree is left untouched.
#[test]
fn precondition_failure_untouched() {
    let tmp = stage_before();
    let project = tmp.path();
    fs::remove_file(project.join("adapters/code-typescript/briefs/survey.md")).unwrap();

    // `plan` never reads brief bodies, so it still succeeds; the missing
    // file only surfaces when `apply` resolves the move precondition.
    let plan = V1ToV2.plan(project).expect("plan still succeeds");
    let err = V1ToV2.apply(project, &plan).expect_err("missing move source aborts apply");
    assert!(matches!(
        err,
        Error::Filesystem {
            op: "migrate-read-source",
            ..
        }
    ));

    assert!(
        project.join("adapters/code-typescript/adapter.yaml").is_file(),
        "monolithic manifest must survive a failed apply",
    );
    assert!(
        !project.join("adapters/sources/code-typescript/adapter.yaml").exists(),
        "no axis-split manifest may land on a failed apply",
    );
    assert!(
        fs::read_to_string(project.join("change.md")).unwrap().contains("/change:"),
        "note rewrite must not commit on a failed apply",
    );
    assert!(!project.join(".specify/.migrate").exists(), "no staging on precondition failure");
}

/// Re-running the migrator over the already-2.0 `after/` tree yields an
/// empty plan and a `Skipped` apply that touches nothing (idempotence).
#[test]
fn already_v2_tree_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path();
    copy_dir_recursive(&fixture_dir("after"), project);

    let before = collect_files(project);

    let plan = V1ToV2.plan(project).expect("plan");
    assert!(plan.actions.is_empty(), "2.0 tree must yield an empty plan, got {:?}", plan.actions);

    let report = V1ToV2.apply(project, &plan).expect("apply");
    assert_eq!(report.status, MigrationStatus::Skipped);

    assert_eq!(before, collect_files(project), "no-op apply must not mutate the tree");
}

#[test]
fn registry_resolves_v1_to_v2() {
    assert_eq!(migrator_for(MigrationKind::V1ToV2).id(), "v1-to-v2");
    assert_eq!(V1ToV2.id(), "v1-to-v2");
}

/// The read-only `probe` over a v1 tree yields the `V1ToV2` hop with a
/// non-empty plan — the data behind `init --check-migration`'s
/// `needs-migration: true`.
#[test]
fn probe_v1_tree_reports_actions() {
    let tmp = stage_before();
    let probed = probe(tmp.path(), 1, 2).expect("probe succeeds");
    assert_eq!(probed.len(), 1, "one registered hop for 1 -> 2");
    assert_eq!(probed[0].kind, MigrationKind::V1ToV2);
    assert!(!probed[0].plan.actions.is_empty(), "v1 tree must have planned actions");
}

/// Probing an already-2.0 tree still resolves the hop but returns an
/// empty plan, so the command layer reports `needs-migration: false`.
#[test]
fn probe_v2_tree_reports_no_actions() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir_recursive(&fixture_dir("after"), tmp.path());
    let probed = probe(tmp.path(), 1, 2).expect("probe succeeds");
    assert_eq!(probed.len(), 1, "the 1 -> 2 hop is still registered");
    assert!(probed[0].plan.actions.is_empty(), "already-2.0 tree plans nothing");
}

/// No registered hop reaches the target (`to <= from` or an unbridged
/// major), so `probe` returns an empty vector.
#[test]
fn probe_same_major_is_empty() {
    let tmp = stage_before();
    assert!(probe(tmp.path(), 2, 2).expect("probe").is_empty());
    assert!(probe(tmp.path(), 1, 1).expect("probe").is_empty());
}
